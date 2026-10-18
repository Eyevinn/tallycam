# Tenants: several customers on one Strom

Status: sketch, 2026-10-18. Nothing here is built. It covers Strom (and the
TallyCam hub it embeds) and Open Live, the case being several Open Live
customers sharing one Strom.

## Today

Isolation is per process, and every process is one customer:

- **Open Live** has no tenant model. On OSC each customer gets their own
  `eyevinn-open-live` instance, with one CouchDB database (`open-live`, no
  per-tenant databases) and one `API_KEY`. A production is a document with no
  owner. The instance talks to one Strom (`STROM_URL`) with one credential
  (`STROM_AUTH_TOKEN`, or an OSC PAT exchanged for a SAT for `eyevinn-strom`).
- **Strom** has one admin login, one `STROM_API_KEY` and a GUI token
  (`backend/src/auth.rs`). An authenticated caller sees and changes every flow,
  port and camera. Two things look tenant-like but authenticate nothing:
  - the port pool's `owner_id`, a free string (Open Live sends its
    `PUBLIC_BASE_URL` host). `docs/PORT_POOL.md` says it plainly: "A port pool
    does not make a shared Strom multi-tenant … Tenant isolation is a separate
    problem."
  - `credential_key` (`client_auth.rs`), which picks the OSC PAT for
    *outbound* calls, per flow.
- **Cameras in Strom** run on one `tallycam_hub::Hub` per Strom
  (`backend/src/cameras/mod.rs`). That means one camera list, one invite pool,
  one `cameras.json` and one production setting (mains, shutter) for everyone.
  - Every controller (`/api/cameras/control`, the `/cameras` page) gets every
    camera's previews, image statistics and state, and can set tally, go
    live, shade or forget any of them.
  - Any flow can bind any camera.
  - The device endpoint `/api/cameras/device` is public; a phone gets in
    with an invite or its device key.

So a Strom shared by two Open Live customers leaks, between them, flows, ports,
cameras, preview pictures, tally and production settings. That is acceptable
only while one Strom serves one customer.

## Goal

One Strom, several customers, each seeing and changing only their own
things. Today's single-customer setups must keep working with no change to
their configuration.

Not goals:
- Making Open Live itself multi-tenant. It stays one instance per customer;
  the tenant boundary that matters is Strom.
- Per-user rights inside a tenant.

## The tenant key

A **tenant** is one customer: in practice one Open Live instance (one OSC
workspace). Strom learns the tenant from the credential a caller presents:

| Credential | Tenant |
|---|---|
| A tenant API key (new: one per tenant, stored hashed) | that tenant |
| An OSC SAT, verified by Strom (later, see below) | its `tenantId` claim |
| The admin login, `STROM_API_KEY`, the GUI token | the admin: every tenant, and the `default` tenant for its own actions |

The auth middleware puts a `Tenant` on every request (`Tenant::Admin` or
`Tenant::Id(..)`). Every handler scopes by it; nothing takes a tenant from a
request parameter. That is the rule that makes this safe: the tenant comes
from the credential, never from what the caller says.

Tenants are configured on the Strom side (a `[[tenants]]` table, or an admin
API: `POST /api/tenants` returns a key once). Open Live gets its tenant key in
`STROM_AUTH_TOKEN` with `STROM_AUTH_MODE=direct`, as today. No Open Live code
changes are needed for the API to become tenant scoped.

**OSC, later:** on OSC, Open Live already holds a SAT. If Strom verified SATs
itself (the OSC JWKS) and read `tenantId`, a customer's Open Live would need no
separate Strom key at all. That needs OSC to issue a SAT whose audience is the
shared Strom. Static tenant keys work without it, so they come first.

## What becomes tenant scoped in Strom

- **Flows** get a `tenant_id`. List, get, change, start and stop only see the
  caller's tenant; the admin sees all. Existing flows belong to `default`.
- **Port pool:** `owner_id` becomes the authenticated tenant (a given
  `owner_id` from a tenant caller is ignored or must match). This closes the
  hole PORT_POOL.md describes.
- **OSC PATs** (`/api/osc/pat/{key}`) are namespaced by tenant, so one tenant
  cannot overwrite another's outbound credential.
- **Events** (`/api/ws`): each subscriber gets only its tenant's flow events.
- **WHEP/WHIP endpoints and media ports:** owned by a flow, so they follow the
  flow's tenant. The passphrases and stream ids that Strom generates per
  binding stay secrets per flow.
- **Quotas** per tenant (flows, ports, cameras), so one customer cannot use up
  the others' share.

## Cameras: one hub per tenant

Strom's `Cameras` holds a map, tenant → `Arc<Hub>`, instead of one hub.
Each tenant's hub is the same TallyCam hub as today, so isolation comes from
construction rather than from filters:

```
phone ──wss──▶ /api/cameras/t/{tenant}/device ──▶ Hub[tenant] ◀── /api/cameras/control
                (public; invite or device key)                    (tenant from the credential)
                                                     │
                                       cameras/{tenant}.json, invites, production
```

- **Device endpoint:** `/api/cameras/t/{tenant}/device`. The tenant in the
  path only picks the hub; it grants nothing. A phone still needs that hub's
  invite or a key bound in that hub, and an unknown tenant is refused. The
  invite QR code (`POST /api/cameras/invite`, scoped to the caller's tenant)
  carries the path, so the phone needs no change. The old
  `/api/cameras/device` maps to `default`.
- **Controller endpoint and the camera API** (`/api/cameras`, forget,
  invite, `/cameras` page): the caller's tenant's hub only. The admin can pick
  a tenant.
- **Store:** `cameras/{tenant}.json`. Today's `cameras.json` becomes
  `default`'s.
- **Bindings:** a flow binds only cameras in its own tenant's hub. The
  `camera` property is looked up in `Hub[flow.tenant]`.
- **Tally** is worked out per flow already; it goes to the flow's tenant's
  hub.
- **Production** (mains, shutter): per hub at least. SHADING.md D6 wants it per
  flow, which needs the hub to take a production per camera (the cameras
  bound to a flow get that flow's settings). That hub change is wanted
  anyway, tenants or not.
- **Device keys and identity:** each hub has its own server id, so a
  phone's key proves itself per hub. A phone serves one server at a time, so
  moving a phone to another tenant is "forget, then scan the other tenant's
  invite", as today.

What the TallyCam hub library needs:
- **No process-wide state.** `MAX_UNADMITTED` in `server.rs` is a static
  today and must become per hub. Strom keeps its own global cap in front of
  the device route.
- **Per-camera production** (above).
- Everything else already works per `Hub` instance.

## Migration

- With no tenants configured, everything belongs to `default` and the
  existing credentials are the admin: today's behaviour, unchanged.
- Adding a tenant: create it, hand its key to that customer's Open Live
  (`STROM_AUTH_TOKEN`), and re-invite its cameras from the new tenant's
  `/cameras` page. Moving existing flows to a tenant is an admin action.
- Open Live needs no change. Port reservations simply stop needing a
  self-chosen `owner_id`.

## Phases

1. **Hub library:** no statics; per-camera production. Small, and it helps
   single-tenant setups too (D6). **Done** (the unadmitted cap is per hub;
   `Hub::set_device_production`).
2. **Strom auth:** tenant keys, `Tenant` on every request, `tenant_id` on
   flows, a scoped port pool, scoped events, namespaced OSC PATs.
3. **Strom cameras:** a hub per tenant, a device path per tenant, a store per
   tenant, bindings checked against the flow's tenant.
4. **OSC:** Strom verifies SATs and reads `tenantId` (once OSC supports a
   shared-Strom audience).

## Open questions

- Is a shared Strom across OSC customers actually planned, or only shared
  across several productions of one customer? Open Live's port-pool code
  already expects several instances on one Strom.
- Tenant keys in Strom's config file versus an admin API and store.
- Admin view: one `/cameras` page per tenant, or one page with a tenant picker
  for the admin.
- Naming: OSC calls it a workspace in the UI and `tenantId` in its tokens.
- Quotas: which limits, and their defaults.
