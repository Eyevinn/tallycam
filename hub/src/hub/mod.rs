//! Hub logic: device registry, controller fan-out, tally and preview gating,
//! and shading (production, image statistics).
//!
//! Transport independent. The WebSocket glue (`server.rs`, or the embedding
//! server's own) feeds parsed messages in and drains the per-device channel
//! and the event broadcast. All state sits behind one `std::sync::Mutex` that
//! is never held across an `.await`; each event is serialized once and
//! broadcast typed and as JSON ([`EventFrame`]).

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::{debug, info, warn};

use crate::device_key::{KeyError, PublicKey};
use crate::protocol::{
    CommandDescriptor, CommandResult, ControlDescriptor, ControllerEvent, ControllerRequest,
    DeviceInfo, DeviceMessage, DeviceSnapshot, Hello, ImageStats, Preview, Production,
    ServerToDevice, Status, TallyState, Values, PROTOCOL_VERSION,
};
use crate::store::{DeviceStore, NoStore, StoredDevice};

mod admission;
mod calls;
mod redact;
mod shading;
#[cfg(test)]
mod tests;

pub(crate) use admission::random_token;
pub use admission::{check_hello_key, Admission, AdmitError, Challenge, Proof};
pub use calls::CallError;
use calls::PendingCall;
use shading::{clamp_stats_fps, StatsPref};
pub use shading::{default_production, DEFAULT_STATS_FPS, SHADING_PROTOCOL};

/// Channel to one connected phone, holding at most [`DEVICE_QUEUE`]
/// messages: a phone that stops reading loses messages instead of growing
/// the hub's memory (and its socket is closed, see `server.rs`).
pub type DeviceTx = mpsc::Sender<ServerToDevice>;
/// Messages queued for one phone, at most.
pub const DEVICE_QUEUE: usize = 256;
/// Devices the hub lists, at most; a new one beyond is refused
/// ([`AdmitError::TooManyDevices`]).
pub const MAX_DEVICES: usize = 1024;
/// Control values kept per device, at most; new keys beyond are dropped.
pub(crate) const MAX_STATE_KEYS: usize = 512;

/// One controller event, typed for an embedding server that maps it to its
/// own events, and as JSON for LCP controllers. Cloning is cheap.
#[derive(Debug, Clone)]
pub struct EventFrame {
    pub event: Arc<ControllerEvent>,
    pub json: Arc<str>,
}

const EVENT_CAPACITY: usize = 512;

/// How long [`Hub::set`] and [`Hub::command`] wait for the phone's `result`
/// when the caller does not say.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(5);
/// How long an invite token from [`Hub::create_invite`] stays valid when the
/// caller does not say.
pub const DEFAULT_INVITE_TTL: Duration = Duration::from_secs(15 * 60);
/// Preview images per second the hub asks each phone for while a controller
/// is connected, unless set with [`Hub::set_preview_fps`] or by a controller.
/// About 15 to 30 kB per 480-wide JPEG, so 0.25 to 0.5 Mbit/s per camera.
pub const DEFAULT_PREVIEW_FPS: f64 = 2.0;
/// Prefix of the ids the hub puts on its own `set` and `command` messages, so
/// they never collide with ids chosen by LCP controllers (numbers).
const CALL_ID_PREFIX: &str = "hub-";

/// Preview settings a controller asked for, per device. Applied only while at
/// least one controller is connected.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PreviewPref {
    enabled: bool,
    fps: f64,
    width: u32,
}

impl Default for PreviewPref {
    fn default() -> Self {
        Self {
            enabled: true,
            fps: DEFAULT_PREVIEW_FPS,
            width: 480,
        }
    }
}

struct Connection {
    id: u64,
    tx: DeviceTx,
    /// The host the phone reached the server by (`Host` header), if known.
    host: Option<String>,
}

struct DeviceEntry {
    info: DeviceInfo,
    /// The device key (base64 SEC1), once a device proved it. `None` for a
    /// device admitted without one (an app before device keys, open hub).
    /// Set only through [`DeviceEntry::set_key`], which keeps `key` in step.
    public_key: Option<String>,
    /// `public_key` parsed, with its fingerprint: parsed once when bound,
    /// not on every snapshot or connect (both run under the hub lock).
    key: Option<(PublicKey, String)>,
    controls: Vec<ControlDescriptor>,
    commands: Vec<CommandDescriptor>,
    state: Values,
    status: Option<Status>,
    preview: Option<Preview>,
    tally: TallyState,
    /// `hello.protocol`; 2 and up know production and stats.
    protocol: u32,
    /// The latest `image_stats`, while the device is online and sends them.
    image_stats: Option<ImageStats>,
    preview_pref: PreviewPref,
    stats_pref: StatsPref,
    conn: Option<Connection>,
}

impl DeviceEntry {
    fn new(info: DeviceInfo, public_key: Option<String>) -> Self {
        let mut entry = Self {
            info,
            public_key: None,
            key: None,
            controls: Vec::new(),
            commands: Vec::new(),
            state: Values::new(),
            status: None,
            preview: None,
            tally: TallyState::Off,
            protocol: 0,
            image_stats: None,
            preview_pref: PreviewPref::default(),
            stats_pref: StatsPref::default(),
            conn: None,
        };
        entry.set_key(public_key);
        entry
    }

    fn snapshot(&self) -> DeviceSnapshot {
        DeviceSnapshot {
            id: self.info.id.clone(),
            info: self.info.clone(),
            controls: self.controls.clone(),
            commands: self.commands.clone(),
            state: redact::for_controllers(&self.state),
            status: self.status.clone(),
            preview: self.preview.clone(),
            tally: self.tally,
            protocol: self.protocol,
            image_stats: self.image_stats.clone(),
            online: self.conn.is_some(),
            key_fingerprint: self.key_fingerprint(),
        }
    }

    fn set_key(&mut self, public_key: Option<String>) {
        self.key = public_key.as_deref().and_then(|k| {
            let parsed = PublicKey::from_base64(k).ok()?;
            Some((parsed, crate::device_key::fingerprint(k)?))
        });
        self.public_key = public_key;
    }

    fn key_fingerprint(&self) -> Option<String> {
        self.key.as_ref().map(|(_, fp)| fp.clone())
    }

    fn stored(&self) -> StoredDevice {
        StoredDevice {
            info: self.info.clone(),
            public_key: self.public_key.clone(),
        }
    }

    fn send(&self, msg: ServerToDevice) -> bool {
        match &self.conn {
            Some(c) => match c.tx.try_send(msg) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    debug!("Device {}: queue full, message dropped", self.info.id);
                    false
                }
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            },
            None => false,
        }
    }

    fn preview_command(&self, controllers: usize) -> ServerToDevice {
        let p = self.preview_pref;
        if controllers > 0 && p.enabled {
            ServerToDevice::Preview {
                enabled: true,
                fps: Some(p.fps),
                width: Some(p.width),
            }
        } else {
            ServerToDevice::Preview {
                enabled: false,
                fps: None,
                width: None,
            }
        }
    }
}

struct Inner {
    /// Keyed by `device.id` from `hello`.
    devices: HashMap<String, DeviceEntry>,
    /// Insertion order for a stable card order in the UI.
    order: Vec<String>,
    controllers: usize,
    /// Typed calls waiting for the phone's `result`, by call id.
    pending: HashMap<String, PendingCall>,
    /// When set, an unknown device needs a valid invite token and a device
    /// key to say hello.
    invite_required: bool,
    /// Unused invite tokens and when they expire.
    invites: HashMap<String, Instant>,
    /// How often each device was forgotten; see [`Admission`].
    forgets: HashMap<String, u64>,
    /// [`Hub::set_preview_fps`]; `None` is [`DEFAULT_PREVIEW_FPS`].
    preview_fps: Option<f64>,
    /// Sent to every protocol 2 device; see [`Hub::set_production`].
    production: Production,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            devices: HashMap::new(),
            order: Vec::new(),
            controllers: 0,
            pending: HashMap::new(),
            invite_required: false,
            invites: HashMap::new(),
            forgets: HashMap::new(),
            preview_fps: None,
            production: default_production(),
        }
    }
}

impl Inner {
    fn forgets(&self, device: &str) -> u64 {
        self.forgets.get(device).copied().unwrap_or(0)
    }

    /// Fail every call waiting on `device` (its connection is gone).
    fn drop_pending_for(&mut self, device: &str) {
        self.pending.retain(|_, p| p.device != device);
    }
}

pub struct Hub {
    inner: Mutex<Inner>,
    /// Random per hub instance, in every `challenge` (and so in what devices
    /// sign).
    server_id: String,
    events: broadcast::Sender<EventFrame>,
    next_conn: AtomicU64,
    next_call: AtomicU64,
    store: Arc<dyn DeviceStore>,
}

impl Default for Hub {
    fn default() -> Self {
        Self::new()
    }
}

/// Keeps a controller counted while alive. Dropping it may disable previews.
pub struct ControllerGuard {
    hub: Arc<Hub>,
}

impl Drop for ControllerGuard {
    fn drop(&mut self) {
        self.hub.controller_left();
    }
}

impl Hub {
    /// A hub that forgets its devices on restart.
    pub fn new() -> Self {
        Self::with_store(Arc::new(NoStore))
    }

    /// A hub that lists the devices in `store` (offline) from the start and
    /// saves every device that says hello. The production starts as the
    /// store kept it, or [`default_production`] when it kept none or one
    /// [`Hub::set_production`] would refuse (an edited state file).
    pub fn with_store(store: Arc<dyn DeviceStore>) -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let mut inner = Inner::default();
        if let Some(production) = store.load_production() {
            match shading::check_production(&production) {
                Ok(()) => inner.production = production,
                Err(e) => warn!("Ignoring the stored production: {e}"),
            }
        }
        for StoredDevice { info, public_key } in store.load() {
            let id = info.id.clone();
            if id.is_empty() || inner.devices.contains_key(&id) {
                continue;
            }
            // A stored key that does not match the id is ignored: the device
            // then has to be invited again, as one stored without a key.
            let public_key = public_key.filter(|k| {
                let ok = crate::device_key::device_id(k).as_deref() == Some(id.as_str());
                if !ok {
                    warn!("Stored key of device {id} does not match its id, ignoring the key");
                }
                ok
            });
            inner.order.push(id.clone());
            inner.devices.insert(id, DeviceEntry::new(info, public_key));
        }
        Self {
            inner: Mutex::new(inner),
            server_id: random_token(),
            events,
            next_conn: AtomicU64::new(1),
            next_call: AtomicU64::new(1),
            store,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Serialize and broadcast. Call with the lock held so the order matches
    /// the state changes and snapshots never miss an event.
    fn emit(&self, event: ControllerEvent) {
        match serde_json::to_string(&event) {
            Ok(json) => {
                // No receivers is fine.
                let _ = self.events.send(EventFrame {
                    event: Arc::new(event),
                    json: Arc::from(json),
                });
            }
            Err(e) => warn!("Failed to serialize controller event: {e}"),
        }
    }

    /// Preview images per second for every device, from now on: the listed
    /// ones and those that say hello later (a controller can still ask one
    /// device for another rate). Clamped to 0.1 to 10, the phone's range.
    /// Meant for startup; a connected phone gets it with its next preview
    /// command (a controller connecting, or the device reconnecting).
    pub fn set_preview_fps(&self, fps: f64) {
        let fps = if fps.is_finite() {
            fps.clamp(0.1, 10.0)
        } else {
            DEFAULT_PREVIEW_FPS
        };
        let mut inner = self.lock();
        inner.preview_fps = Some(fps);
        for entry in inner.devices.values_mut() {
            entry.preview_pref.fps = fps;
        }
    }

    pub fn new_connection_id(&self) -> u64 {
        self.next_conn.fetch_add(1, Ordering::Relaxed)
    }

    pub fn controller_count(&self) -> usize {
        self.lock().controllers
    }

    /// Everything the hub knows, in display order (what `snapshot` carries).
    pub fn devices(&self) -> Vec<DeviceSnapshot> {
        let inner = self.lock();
        Self::snapshot_list(&inner)
    }

    /// Events from now on, for an embedding server that does not count as an
    /// LCP controller (it never enables previews). Pair it with [`Self::devices`]
    /// for the starting state.
    pub fn subscribe(&self) -> broadcast::Receiver<EventFrame> {
        self.events.subscribe()
    }

    // -----------------------------------------------------------------------
    // Devices
    // -----------------------------------------------------------------------

    /// The first `hello` of connection `conn`, after [`Self::admit`] let it
    /// in. A new connection for a known id replaces the old one (the old
    /// socket closes when its channel drops) and keeps the last-known state
    /// and tally. `host` is the `Host` of the phone's WebSocket request, kept
    /// for [`Self::device_host`]. Refused when the device was forgotten after
    /// it was admitted.
    pub fn device_join(
        &self,
        conn: u64,
        hello: Hello,
        admission: Admission,
        tx: DeviceTx,
        host: Option<String>,
    ) -> Result<(), AdmitError> {
        let mut inner = self.lock();
        if inner.forgets(&hello.device.id) != admission.forgets {
            return Err(AdmitError::NoInvite);
        }
        self.hello_locked(
            &mut inner,
            conn,
            hello,
            Some(admission.key),
            Some((tx, host)),
        );
        Ok(())
    }

    /// A `hello` resent on the admitted connection `conn` (new name or
    /// controls). It never changes the device's key and is ignored for any
    /// other connection.
    pub fn device_hello(&self, conn: u64, hello: Hello) {
        let mut inner = self.lock();
        let current = inner
            .devices
            .get(&hello.device.id)
            .and_then(|e| e.conn.as_ref())
            .is_some_and(|c| c.id == conn);
        if current {
            self.hello_locked(&mut inner, conn, hello, None, None);
        }
    }

    /// `key`: the admitted key to bind (`Some(None)`: admitted without one);
    /// `None` for a resent hello. `link`: the new connection's channel and
    /// host, `None` for a resent hello. Saves to the store under the lock,
    /// so `forget` cannot interleave.
    fn hello_locked(
        &self,
        inner: &mut MutexGuard<'_, Inner>,
        conn: u64,
        hello: Hello,
        key: Option<Option<String>>,
        link: Option<(DeviceTx, Option<String>)>,
    ) {
        let controllers = inner.controllers;
        let production = inner.production.clone();
        let id = hello.device.id.clone();
        // Older devices are fine (version 1 has no production); a newer one
        // may use messages this hub does not know.
        if hello.protocol > PROTOCOL_VERSION {
            warn!(
                "Device {id} speaks protocol {}, hub speaks {PROTOCOL_VERSION}",
                hello.protocol
            );
        }
        if !inner.devices.contains_key(&id) {
            inner.order.push(id.clone());
        }
        let mut new_device = false;
        let inner_preview_fps = inner.preview_fps.unwrap_or(DEFAULT_PREVIEW_FPS);
        let entry = inner.devices.entry(id.clone()).or_insert_with(|| {
            new_device = true;
            let mut entry = DeviceEntry::new(hello.device.clone(), None);
            entry.preview_pref.fps = inner_preview_fps;
            entry
        });
        // Only the key `admit` verified is bound; the key a hello carries is
        // never taken from it, and an admission without a key keeps the one
        // that is bound.
        let key_changed = matches!(&key, Some(Some(k)) if entry.public_key.as_ref() != Some(k));
        if key_changed {
            entry.set_key(key.flatten());
        }
        let changed = new_device || key_changed || entry.info != hello.device;
        entry.info = hello.device;
        entry.protocol = hello.protocol;
        if !entry.shading() {
            // An older app on the same device sends no statistics.
            entry.image_stats = None;
        }
        let save = changed.then(|| entry.stored());
        entry.controls = hello.controls;
        entry.commands = hello.commands;

        if let Some((tx, host)) = link {
            if entry.conn.is_some() {
                info!("Device {id} reconnected, replacing the old connection");
            } else {
                info!("Device {id} ({}) online", entry.info.name);
            }
            entry.conn = Some(Connection { id: conn, tx, host });
            // Bring the phone up to date with what the hub owns.
            entry.send(ServerToDevice::Tally { state: entry.tally });
            entry.send(entry.preview_command(controllers));
            if entry.shading() {
                entry.send(ServerToDevice::Production(production));
                entry.send_stats(controllers);
            }
            // Calls sent on the old connection will not be answered.
            inner.drop_pending_for(&id);
        } else {
            debug!("Device {id} resent hello");
        }
        if let Some(device) = save {
            self.store.save(&device);
        }
        let entry = inner.devices.get(&id).expect("inserted above");

        let event = ControllerEvent::DeviceHello {
            device: id,
            info: entry.info.clone(),
            controls: entry.controls.clone(),
            commands: entry.commands.clone(),
            protocol: entry.protocol,
            key_fingerprint: entry.key_fingerprint(),
        };
        self.emit(event);
    }

    /// Any message after `hello`. Ignored if `conn` is no longer the current
    /// connection for `device_id` (a stale socket after a reconnect).
    pub fn device_message(&self, conn: u64, device_id: &str, msg: DeviceMessage) {
        let mut guard = self.lock();
        let inner = &mut *guard;
        let Some(entry) = inner.devices.get_mut(device_id) else {
            return;
        };
        if entry.conn.as_ref().map(|c| c.id) != Some(conn) {
            return;
        }
        let device = device_id.to_string();
        let event = match msg {
            DeviceMessage::Hello(_) => return, // handled by device_hello
            DeviceMessage::State { values } => {
                for (k, v) in &values {
                    if entry.state.len() < MAX_STATE_KEYS || entry.state.contains_key(k) {
                        entry.state.insert(k.clone(), v.clone());
                    }
                }
                ControllerEvent::DeviceState {
                    device,
                    values: redact::for_controllers(&values),
                }
            }
            DeviceMessage::Status(status) => {
                entry.status = Some(status.clone());
                ControllerEvent::DeviceStatus { device, status }
            }
            DeviceMessage::Preview(preview) => {
                entry.preview = Some(preview.clone());
                ControllerEvent::DevicePreview { device, preview }
            }
            DeviceMessage::Result(result) => {
                // A result for one of the hub's own calls goes to the caller;
                // controllers still see it, like any other result.
                if let Some(id) = result.id.as_str() {
                    if inner.pending.get(id).is_some_and(|p| p.device == device) {
                        if let Some(p) = inner.pending.remove(id) {
                            let _ = p.tx.send(result.clone());
                        }
                    }
                }
                ControllerEvent::DeviceResult { device, result }
            }
            DeviceMessage::Log(log) => ControllerEvent::DeviceLog { device, log },
            DeviceMessage::ImageStats(stats) => {
                entry.image_stats = Some(stats.clone());
                ControllerEvent::DeviceImageStats { device, stats }
            }
            DeviceMessage::Ping(p) => {
                entry.send(ServerToDevice::Pong(p));
                return;
            }
            DeviceMessage::Pong(_) | DeviceMessage::Auth { .. } | DeviceMessage::Unknown => return,
        };
        self.emit(event);
    }

    /// The socket for `conn` closed or timed out.
    pub fn device_disconnected(&self, conn: u64, device_id: &str) {
        let mut inner = self.lock();
        let Some(entry) = inner.devices.get_mut(device_id) else {
            return;
        };
        if entry.conn.as_ref().map(|c| c.id) != Some(conn) {
            return; // replaced by a newer connection
        }
        entry.conn = None;
        // Live data from the old connection would read as current after a
        // reconnect (a stream counted live that is not).
        entry.status = None;
        entry.image_stats = None;
        inner.drop_pending_for(device_id);
        info!("Device {device_id} offline");
        self.emit(ControllerEvent::DeviceOffline {
            device: device_id.to_string(),
        });
    }

    // -----------------------------------------------------------------------
    // Controllers
    // -----------------------------------------------------------------------

    fn snapshot_list(inner: &Inner) -> Vec<DeviceSnapshot> {
        inner
            .order
            .iter()
            .filter_map(|id| inner.devices.get(id))
            .map(DeviceEntry::snapshot)
            .collect()
    }

    fn snapshot_json(inner: &Inner) -> String {
        serde_json::to_string(&ControllerEvent::Snapshot {
            protocol: PROTOCOL_VERSION,
            devices: Self::snapshot_list(inner),
            production: inner.production.clone(),
        })
        .unwrap_or_else(|_| "{}".into())
    }

    /// Register a controller. Returns the snapshot to send first, the event
    /// stream that continues exactly after it, and a guard that unregisters.
    pub fn controller_joined(
        self: &Arc<Self>,
    ) -> (String, broadcast::Receiver<EventFrame>, ControllerGuard) {
        let mut inner = self.lock();
        inner.controllers += 1;
        if inner.controllers == 1 {
            info!("First controller connected, enabling previews and image stats");
            for entry in inner.devices.values_mut() {
                entry.send(entry.preview_command(1));
                entry.send_stats(1);
            }
        }
        let snapshot = Self::snapshot_json(&inner);
        let rx = self.events.subscribe();
        (
            snapshot,
            rx,
            ControllerGuard {
                hub: Arc::clone(self),
            },
        )
    }

    /// A fresh snapshot and receiver for a controller that lagged behind.
    pub fn resync(&self) -> (String, broadcast::Receiver<EventFrame>) {
        let inner = self.lock();
        (Self::snapshot_json(&inner), self.events.subscribe())
    }

    fn controller_left(&self) {
        let mut inner = self.lock();
        inner.controllers = inner.controllers.saturating_sub(1);
        if inner.controllers == 0 {
            info!("Last controller disconnected, disabling previews and image stats");
            for entry in inner.devices.values_mut() {
                entry.send(entry.preview_command(0));
                entry.send_stats(0);
            }
        }
    }

    /// Set a device's tally: from an operator, or from the production (a
    /// vision mixer's program and preview). Kept while the device is offline
    /// and sent when it connects, so a camera that drops off the network
    /// while on air comes back lit.
    pub fn set_tally(&self, device: &str, state: TallyState) -> Result<(), String> {
        let mut inner = self.lock();
        let Some(entry) = inner.devices.get_mut(device) else {
            return Err(format!("unknown device '{device}'"));
        };
        self.apply_tally(entry, device, state);
        Ok(())
    }

    /// Only changes reach the phone and the controllers, so a production
    /// that repeats its tally on every cut costs nothing.
    fn apply_tally(&self, entry: &mut DeviceEntry, device: &str, state: TallyState) {
        if entry.tally == state {
            return;
        }
        entry.tally = state;
        entry.send(ServerToDevice::Tally { state });
        self.emit(ControllerEvent::DeviceTally {
            device: device.to_string(),
            state,
        });
    }

    /// The host a connected phone reached the server by (the `Host` header
    /// of its WebSocket request, port included). A server that hands the
    /// phone a media URL can use the same name: the phone can resolve it.
    pub fn device_host(&self, device: &str) -> Option<String> {
        self.lock()
            .devices
            .get(device)
            .and_then(|e| e.conn.as_ref())
            .and_then(|c| c.host.clone())
    }

    /// Is the device connected now?
    pub fn is_online(&self, device: &str) -> bool {
        self.lock()
            .devices
            .get(device)
            .is_some_and(|e| e.conn.is_some())
    }

    /// The latest control values a phone reported (`state`), empty for an
    /// unknown device. Unredacted (the SRT passphrase included, unlike in
    /// snapshots and events): for the embedding server's own use, never to
    /// be passed on to an API or a controller as is.
    pub fn device_state(&self, device: &str) -> Values {
        self.lock()
            .devices
            .get(device)
            .map(|e| e.state.clone())
            .unwrap_or_default()
    }

    /// Handle a controller request. `Err` goes back to that controller only.
    /// Exposure, colour and focus are not guarded: the last write wins,
    /// whether it comes from a controller or the camera operator.
    pub fn controller_request(&self, req: ControllerRequest) -> Result<(), String> {
        let mut inner = self.lock();
        let controllers = inner.controllers;
        let Some(entry) = inner.devices.get_mut(&req.device) else {
            return Err(format!("unknown device '{}'", req.device));
        };
        match req.message {
            ServerToDevice::Tally { state } => {
                self.apply_tally(entry, &req.device, state);
                Ok(())
            }
            ServerToDevice::Preview {
                enabled,
                fps,
                width,
            } => {
                let pref = PreviewPref {
                    enabled,
                    fps: fps.filter(|f| *f > 0.0).unwrap_or(entry.preview_pref.fps),
                    width: width.filter(|w| *w > 0).unwrap_or(entry.preview_pref.width),
                };
                entry.preview_pref = pref;
                entry.send(entry.preview_command(controllers));
                Ok(())
            }
            ServerToDevice::Stats { enabled, fps } => {
                entry.stats_pref = StatsPref {
                    enabled,
                    fps: fps
                        .filter(|f| f.is_finite() && *f > 0.0)
                        .map(clamp_stats_fps)
                        .unwrap_or(entry.stats_pref.fps),
                };
                entry.send_stats(controllers);
                Ok(())
            }
            ServerToDevice::Ping(_) | ServerToDevice::Pong(_) => Ok(()),
            ServerToDevice::Challenge { .. } | ServerToDevice::Production(_) => {
                Err("not a controller request".into())
            }
            mut msg @ (ServerToDevice::Set { .. }
            | ServerToDevice::Command { .. }
            | ServerToDevice::Message { .. }) => {
                if entry.conn.is_none() {
                    return Err(format!("device '{}' is offline", req.device));
                }
                if let ServerToDevice::Set { values, .. } = &mut msg {
                    redact::restore(values, &entry.state);
                }
                if !entry.send(msg) {
                    return Err(format!("device '{}' connection closed", req.device));
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod preview_fps_tests {
    use super::*;

    struct OneDevice;

    impl DeviceStore for OneDevice {
        fn load(&self) -> Vec<StoredDevice> {
            vec![StoredDevice {
                info: serde_json::from_value(serde_json::json!({ "id": "a", "name": "Cam A" }))
                    .unwrap(),
                public_key: None,
            }]
        }
        fn save(&self, _device: &StoredDevice) {}
    }

    fn fps(hub: &Hub, id: &str) -> Option<f64> {
        match hub.lock().devices[id].preview_command(1) {
            ServerToDevice::Preview { fps, .. } => fps,
            other => panic!("not a preview command: {other:?}"),
        }
    }

    #[test]
    fn preview_fps_defaults_and_applies_to_listed_and_new_devices() {
        let hub = Hub::with_store(Arc::new(OneDevice));
        assert_eq!(fps(&hub, "a"), Some(DEFAULT_PREVIEW_FPS));

        hub.set_preview_fps(5.0);
        assert_eq!(fps(&hub, "a"), Some(5.0));

        // Clamped to the phone's range.
        hub.set_preview_fps(50.0);
        assert_eq!(fps(&hub, "a"), Some(10.0));

        // A device that says hello after the setting gets it too.
        {
            let mut inner = hub.lock();
            let hello = Hello {
                protocol: PROTOCOL_VERSION,
                device: serde_json::from_value(serde_json::json!({ "id": "b", "name": "Cam B" }))
                    .unwrap(),
                public_key: None,
                controls: Vec::new(),
                commands: Vec::new(),
            };
            hub.hello_locked(&mut inner, 1, hello, Some(None), None);
        }
        assert_eq!(fps(&hub, "b"), Some(10.0));
    }
}
