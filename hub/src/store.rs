//! Where the hub keeps the devices it has seen, so they are listed (offline)
//! after a restart. The embedding server owns persistence: the standalone hub
//! uses [`JsonFileStore`], Strom plugs in its own storage.
//!
//! Stored: each device's identity (the hello's device info and its public
//! key, once it proved one) and the production settings. Tally, state and
//! status are live data and start empty: a tally kept across a restart would
//! light a camera that is not on air.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::protocol::{DeviceInfo, Production};

/// One known device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StoredDevice {
    pub info: DeviceInfo,
    /// The device key (base64 SEC1 uncompressed P-256). A device stored
    /// without one (admitted before device keys) needs a new invite when
    /// the server requires invites; the key is never bound silently on a
    /// later connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
}

/// Persistence for known devices and the production. The hub calls `save`,
/// `remove` and `save_production` while it holds its own lock (so a forget
/// and a reconnect cannot interleave), from the task that handled the
/// message. They must be quick (a small write, or
/// a send to a writer task), since every other hub operation waits, and must
/// not call back into the hub, which would deadlock.
pub trait DeviceStore: Send + Sync {
    /// Devices to list at startup, in display order.
    fn load(&self) -> Vec<StoredDevice>;
    /// A device said hello (new, or with changed info or key).
    fn save(&self, device: &StoredDevice);
    /// The device was forgotten ([`crate::Hub::forget`]). The default keeps
    /// it, so stores written before `forget` existed still compile; a store
    /// that persists should implement it, or a forgotten device is listed
    /// again after a restart.
    fn remove(&self, _id: &str) {}
    /// The production settings kept by `save_production`, if any. The
    /// default keeps none: a store that persists should implement both, or
    /// the production is back to [`crate::hub::default_production`] after a
    /// restart.
    fn load_production(&self) -> Option<Production> {
        None
    }
    /// The production changed ([`crate::Hub::set_production`]).
    fn save_production(&self, _production: &Production) {}
}

/// Keeps nothing: every restart starts with an empty list.
#[derive(Debug, Default)]
pub struct NoStore;

impl DeviceStore for NoStore {
    fn load(&self) -> Vec<StoredDevice> {
        Vec::new()
    }
    fn save(&self, _device: &StoredDevice) {}
}

/// All devices and the production in one JSON file, rewritten on every
/// change (a hub has tens of devices, not thousands).
pub struct JsonFileStore {
    path: PathBuf,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// Keyed by device id; insertion order is kept in `order`.
    devices: BTreeMap<String, StoredDevice>,
    order: Vec<String>,
    production: Option<Production>,
}

#[derive(Serialize, Deserialize)]
struct FileContents {
    devices: Vec<StoredDevice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    production: Option<Production>,
}

impl JsonFileStore {
    /// Opens `path`. A missing file is an empty store; an unreadable one is
    /// logged and replaced on the first save.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let contents = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<FileContents>(&bytes) {
                Ok(c) => Some(c),
                Err(e) => {
                    warn!("Ignoring device store {}: {e}", path.display());
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                warn!("Cannot read device store {}: {e}", path.display());
                None
            }
        };
        let mut state = State::default();
        if let Some(c) = contents {
            state.production = c.production;
            for d in c.devices {
                if state.devices.insert(d.info.id.clone(), d.clone()).is_none() {
                    state.order.push(d.info.id);
                }
            }
        }
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self, state: &State) {
        let contents = FileContents {
            devices: state
                .order
                .iter()
                .filter_map(|id| state.devices.get(id).cloned())
                .collect(),
            production: state.production.clone(),
        };
        let Ok(json) = serde_json::to_vec_pretty(&contents) else {
            return;
        };
        // Write a sibling file and rename, so a crash never leaves half a file.
        let tmp = self.path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, &self.path))
        {
            warn!("Cannot write device store {}: {e}", self.path.display());
        }
    }
}

impl DeviceStore for JsonFileStore {
    fn load(&self) -> Vec<StoredDevice> {
        let state = self.lock();
        state
            .order
            .iter()
            .filter_map(|id| state.devices.get(id).cloned())
            .collect()
    }

    fn save(&self, device: &StoredDevice) {
        let mut state = self.lock();
        let id = &device.info.id;
        if state.devices.get(id) == Some(device) {
            return;
        }
        if state.devices.insert(id.clone(), device.clone()).is_none() {
            state.order.push(id.clone());
        }
        self.write(&state);
    }

    fn remove(&self, id: &str) {
        let mut state = self.lock();
        if state.devices.remove(id).is_none() {
            return;
        }
        state.order.retain(|o| o != id);
        self.write(&state);
    }

    fn load_production(&self) -> Option<Production> {
        self.lock().production.clone()
    }

    fn save_production(&self, production: &Production) {
        let mut state = self.lock();
        if state.production.as_ref() == Some(production) {
            return;
        }
        state.production = Some(production.clone());
        self.write(&state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store from before `remove` existed: still a `DeviceStore`.
    struct LegacyStore;

    impl DeviceStore for LegacyStore {
        fn load(&self) -> Vec<StoredDevice> {
            Vec::new()
        }
        fn save(&self, _device: &StoredDevice) {}
    }

    #[test]
    fn remove_defaults_to_a_no_op() {
        let store: &dyn DeviceStore = &LegacyStore;
        store.remove("any");
        assert!(store.load().is_empty());
    }

    fn device(id: &str, name: &str) -> StoredDevice {
        let info: DeviceInfo = serde_json::from_value(serde_json::json!({
            "id": id, "name": name, "model": "iPhone", "app_version": "0.1"
        }))
        .unwrap();
        StoredDevice {
            info,
            public_key: None,
        }
    }

    #[test]
    fn json_file_store_round_trips_in_order() {
        let dir = std::env::temp_dir().join(format!("tallycam-hub-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("devices.json");
        let _ = std::fs::remove_file(&path);

        let store = JsonFileStore::open(&path);
        assert!(store.load().is_empty());
        store.save(&device("b", "Cam B"));
        store.save(&device("a", "Cam A"));
        store.save(&device("b", "Cam B renamed"));

        let reopened = JsonFileStore::open(&path);
        let names: Vec<_> = reopened.load().into_iter().map(|d| d.info.name).collect();
        assert_eq!(names, ["Cam B renamed", "Cam A"]);

        let mut keyed = device("c", "Cam C");
        keyed.public_key = Some("BAE=".into());
        reopened.save(&keyed);
        reopened.remove("b");
        let again = JsonFileStore::open(&path);
        let loaded = again.load();
        let ids: Vec<_> = loaded.iter().map(|d| d.info.id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
        assert_eq!(loaded[0].public_key, None);
        assert_eq!(loaded[1].public_key.as_deref(), Some("BAE="));
        assert_eq!(again.load_production(), None);

        let production = Production {
            mains_hz: 60,
            shutter: crate::protocol::ShutterPolicy::Fixed(120),
            extra: Default::default(),
        };
        again.save_production(&production);
        let last = JsonFileStore::open(&path);
        assert_eq!(last.load_production(), Some(production));
        assert_eq!(last.load().len(), 2, "devices kept");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
