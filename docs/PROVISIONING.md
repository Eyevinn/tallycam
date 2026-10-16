# Provisioning v1 (draft for discussion)

Status: design sketch. What the prototype does today is provisioning v0 (a
`tallycam://` deep link carrying the control and SRT URLs, with a one-time
invite token in the control URL where the server requires invites) plus the
**device key**: the phone proves a Secure Enclave key on every connection,
and an invite binds that key. See `PROTOCOL.md`, "Provisioning v0" and
"Device keys". The join API, session tokens, trust levels and per-device
media credentials below are not implemented.

## Goals

- **One invite link, any channel.** A QR code on a poster or a ticket, a link in
  an email, SMS or WhatsApp: it is the same `https` link.
- **Security is in the invite, chosen per use.** A crowd invite at a venue and
  a studio camera enrolment need very different levels of trust.
- **Works whether or not the app is installed.** The link opens the app if it is
  there, and a web page with an App Store link (and later a browser fallback)
  if it is not.
- **The production can run in the cloud** behind Open Live, or on a single
  Strom, without changing the phone side.
- **The link is not the credential.** The invite token is exchanged once for a
  device credential that lives on the phone, so a leaked link or screenshot
  does not give lasting access.

## The invite link

```
https://join.tallycam.example/j#s=prod.example.com&i=7Hk2pQ9xVb3mR8wL
```

- `join.tallycam.example` is **one fixed domain owned by Eyevinn**. iOS
  Universal Links and Android App Links tie a domain to the app at build time,
  so the app cannot claim each customer's domain. The fixed domain only hosts
  the `apple-app-site-association` / `assetlinks.json` files and the fallback
  page.
- Everything after `#` is a **fragment**: the browser never sends it to
  `join.tallycam.example`. The central domain learns nothing about the
  production; only the phone reads it.
  - `s` = the production's join server (Open Live or Strom), host and optional
    port.
  - `i` = the invite token, at least 96 random bits, base32 or base62.
- Fallback when the app is missing: the page (static, no server logic) shows
  the App Store and Google Play links, and stores nothing. On first launch the
  user scans or opens the link again. (A deferred deep link that survives the
  install is possible but needs a server-side lookup; left out of v1.)
- A short form for printing (tickets, posters) may use a shorter token plus a
  PIN; see the levels below.

## Flow

```
phone                                   join server (Open Live or Strom)
  │ open link → app reads s, i
  │ GET  https://s/tallycam/v1/invites/{i}         (preview, no side effects)
  │ ◀─ production name, logo, organiser, level, what is asked (PIN? name?),
  │    terms, expiry
  │ user sees "Join <production> as <level>?" and accepts
  │ POST https://s/tallycam/v1/join
  │      {invite: i, pin?, name?, device: {id, model, os, app_version},
  │       public_key, attestation?}
  │ ◀─ 202 pending (waiting for operator approval) → poll / long-poll
  │ ◀─ 200 {device_id, session_token (short-lived), refresh via signed
  │        challenge, control_url (wss), media: [...], clock: {pools},
  │        level, limits, expires_at}
  │ wss control_url  Authorization: Bearer <session_token>
  │ SRT  streamid + passphrase from media[]
```

- **Device key** (implemented, over the LCP device endpoint rather than a
  join API). The phone creates a P-256 key pair in the Secure Enclave
  (Android: Keystore) and sends the public key in `hello`; its device id is
  derived from the key. Every connection answers a server challenge with a
  signature, so nothing long-lived is a bearer secret. The invite token is
  spent once, binding the key; afterwards the key alone admits the phone
  until an operator forgets it. Open: TLS (without it a man in the middle
  can still relay a challenge or take over an admitted socket), and binding
  the challenge to the URL the phone dialed once there is TLS.
- **Attestation (optional per invite).** Apple App Attest / Google Play
  Integrity prove the request comes from the real TallyCam app on a real
  device. Useful against scripted abuse of open crowd invites.
- **Media credentials are per device and per session.**
  - SRT: the stream id uses SRT's access control syntax,
    `#!::r=<production>/<camera>,u=<device_id>,t=<media_token>`, which the
    receiving side checks before accepting the caller, plus an SRT
    **passphrase** (AES) unique to the device, in the SRT URL's `passphrase`
    parameter (the app's SRT caller encrypts).
  - WHIP (later): a bearer token per device.
- **Revocation.** Revoking an invite stops new joins. Revoking a device closes
  its control channel and its media credentials stop working at the next
  check (the SRT listener rejects the stream id, the passphrase is rotated).

## Trust levels

The invite decides the level. The phone shows it and the user accepts it.

| Level | Typical channel | Who | Token | Extra checks | Device may | Server may |
|---|---|---|---|---|---|---|
| **Open crowd** | QR on a venue poster or screen | Anyone present | Shared by everyone, valid for the event window | Rate limits, optional attestation, moderation before air | Send one stream | Destination, encoding, tally, messages |
| **Ticket** | QR or code printed on a ticket, email to ticket holders | One ticket holder | Unique per ticket, binds to the first device | Optional PIN on the ticket, moderation | Send one stream | Same as open crowd |
| **Invited contributor** | Personal link by email, SMS, WhatsApp | A named guest or reporter | Unique, single use, short expiry | PIN sent by another channel, or the operator approves | Stream, talkback | Plus start/stop, bitrate |
| **Crew camera** | Link or QR from the control page | Camera operator | Unique, single use, minutes | Operator approves on the control page ("iPhone of Anna wants to join as Camera 3") | Stream, talkback | Full camera control, tally |
| **Studio device** | Enrolled once, then reconnects | Fixed or robotic camera | Device key only after enrolment | Operator approval, attestation | Stream | Everything, including PTZ, unattended |

What the levels change:

- **Moderation.** Open crowd and ticket streams land in a pool and are not on
  air until an operator picks them. The production sees name/ticket, preview
  and network quality first.
- **Limits.** Per invite: maximum devices, maximum concurrent streams,
  bitrate cap, expiry. Per network address: join rate.
- **Control.** Crowd levels can never be switched to full camera control from
  the server; that needs a new invite at a higher level and the user's consent.
- **Unattended.** Only a studio device may be started by the production
  without someone at the phone accepting.

## Where it runs

- **Open Live as the public front:** the join server is Open Live. It owns
  users, roles, invites, moderation and productions spanning several Strom
  instances. It hands out media endpoints that point at the right Strom.
- **Strom alone:** Strom implements the same `tallycam/v1` join API with a
  simple invite store (create invite, show QR, approve devices on its control
  page). Same phone behaviour.
- **The central join domain** serves only static files. It is never in the
  data path and never sees an invite token.

## Open questions

- Domain and name for the join link (`join.tallycam.example` is a placeholder).
- Deferred deep links (keep the invite across the App Store install): worth a
  small server-side lookup, or is "open the link again" acceptable?
- Browser fallback for crowd invites (WHIP from the web page) when the app is
  not installed: yes for open crowd, probably not for crew.
- Consent and terms for crowd contribution: who stores the acceptance, and
  what the contributor sees about how their footage is used.
- SRT access control needs a receiver in Strom that validates stream ids per
  device (the phone already sends passphrase-encrypted SRT).
- Offline venues: an on-site Strom as join server when there is no internet.
