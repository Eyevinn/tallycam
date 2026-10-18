# Security

How TallyCam is meant to be run safely, what it protects, and what it does
not. Written after the security review of 2026-10-18 (hub, web pages,
core, apps); the fixes are on main.

## What there is to protect

- **The picture and sound.** A phone streams camera and microphone to the
  SRT URL it is given. Whoever controls that URL, or `live`, controls where
  the picture goes.
- **The SRT passphrase** in the stream URL: it encrypts the stream.
- **Control of the cameras:** tally, go live, exposure and the other
  controls, forget.
- **Admission:** the invite token (one use, 15 minutes), a phone's device
  key (P-256, in the Secure Enclave or Android Keystore, never exported) and
  the operator token of the hub page.

## Who can do what

| Party | Can | Can not |
|---|---|---|
| The hub (or Strom) a phone joined | everything on that phone: controls, go live, send the picture to any SRT URL | admit itself to another hub as that phone (protocol 4 signs the dialled host); see the passphrase of another target |
| An operator with the token (or anyone, on an open hub) | control every camera on that hub, invite and forget | read a camera's passphrase (shown as `***`), or send it to another listener |
| An admitted phone | its own state, status, previews, logs | speak for another camera; grow the hub without bound (sizes are capped) |
| An unadmitted client | open sockets and send one hello | hold more than 64 sockets per hub, longer than 10 s, larger than 256 KiB |
| A web page in the operator's browser | nothing with a token; on an open hub it is refused by Origin and Host checks (no CSRF, no DNS rebinding, no framing) | |
| A link (QR code, web page, other app) | ask the phone to join a hub: the app shows "Join <hub>?" with the hub and SRT hosts, "not encrypted" for ws:// | join silently, downgrade to ws:// or change the SRT target without the prompt; use ws:// to a public host |
| Someone on the network path | read and change everything on a `ws://` link, and the SRT handshake on an unencrypted stream | break `wss://` (system CAs, standard checks) or an SRT stream with a passphrase |

## Protected

- **Admission:** a new phone needs an invite (the default; always in
  Strom; not with `--open`). A known phone proves its key on every connection: a fresh
  256-bit nonce per challenge, a signature over the server id, the nonce,
  the device id and the host it dialled, checked against the host the
  connection reached (`Host`, or `X-Forwarded-Host` behind a proxy).
  Device ids derive from the key.
- **The passphrase** stays between the hub and the phone: controllers see
  `***`, a `***` sent back is restored only for the same target and stream
  id, logs and `/api/info` redact it, and the phone redacts it in status
  and in its own state.
- **The hub page:** the token is checked in constant time, comes from the
  URL fragment (never sent to the server in the URL) and goes over the
  WebSocket as a subprotocol; the HTTP API needs it in a header. Pages send
  a CSP, `X-Frame-Options: DENY`, `nosniff` and `no-referrer`, and render
  device text as text only.
- **Bounds:** WebSocket messages 8 MiB (256 KiB before admission), 1024
  devices per hub, 512 state values of up to 16 KiB, previews up to 1 MiB,
  a queue of 256 messages per phone (one that stops reading is dropped),
  controllers that stop reading are dropped, names without control
  characters.
- **Apps:** only the launcher activity is exported, intents only give a
  link (to the join prompt), numbers from the hub are checked and clamped,
  messages from the hub are size-capped, secrets are excluded from backups
  and device transfer, and release builds never fall back to the debug key.

## Not protected (yet)

- **No TLS in the standalone hub.** `control-hub` serves plain HTTP and
  `ws://`. On a shared network anyone on the path reads the passphrase, the
  operator token and invite tokens, and can drive the phones. Put it behind
  a TLS proxy (`--public-url wss://...`) for anything but a closed network.
  The apps take `ws://` only to local network hosts.
- **An open hub (`--open`)** lets any host that reaches it join a camera
  and control every camera; it says so in a warning at start. The default is
  invite-only with a generated operator token; use `--open` only on a closed
  test network.
- **HTTP connections have no read timeout** in `control-hub` (slow clients
  can hold sockets). Behind a proxy the proxy limits this.
- **Unadmitted sockets are capped per hub, not per address:** one host can
  keep phones from joining for as long as it holds 64 sockets.
- **The SRT stream id is logged in full**; once it carries a media token
  (PROVISIONING.md) that must be redacted like the passphrase.
- **SRT listener identity:** the SRT caller does not verify the listener's
  key material response, and any listener the phone is sent to can try to
  guess the passphrase offline from the handshake. Use long random
  passphrases (Strom does).
- **Several customers on one Strom** are not isolated (docs/TENANTS.md):
  one Strom per customer.

## Running it safely

1. Strom: run it as the docs say, one per customer, behind TLS. Cameras
   always need an invite there.
2. Standalone hub: `control-hub --public-url wss://hub.example/...`
   behind a TLS proxy, with the token from the log (or
   `LCP_HUB_OPERATOR_TOKEN`, which keeps it out of `ps`). Open the page with
   the printed `#token=` link; it removes the token from the address bar.
3. Keep the state file (`--state-file`) in a directory only the hub's user
   can write.
4. Give SRT URLs a passphrase (10 to 79 characters, random).
5. Without TLS, use a network only the production uses.

## Reporting

Report a vulnerability privately to the maintainers (not in a public
issue), with the commit, the steps and what an attacker gains.
