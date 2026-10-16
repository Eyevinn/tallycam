//! Where the hub keeps the devices it has seen, so they are listed (offline)
//! after a restart. The embedding server owns persistence: the standalone hub
//! uses [`JsonFileStore`], Strom plugs in its own storage.
//!
//! Only identity is stored: the hello's device info and the device's public
//! key (once it proved one). Tally, state and status are live data and start
//! empty: a tally kept across a restart would light a camera that is not on
//! air.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::protocol::DeviceInfo;

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

/// Persistence for known devices. The hub calls `save` and `remove` while it
/// holds its own lock (so a forget and a reconnect cannot interleave), from
/// the task that handled the message. They must be quick (a small write, or
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

/// All devices in one JSON file, rewritten on every change (a hub has tens of
/// devices, not thousands).
pub struct JsonFileStore {
    path: PathBuf,
    /// Keyed by device id; insertion order is kept in `order`.
    devices: Mutex<(BTreeMap<String, StoredDevice>, Vec<String>)>,
}

#[derive(Serialize, Deserialize)]
struct FileContents {
    devices: Vec<StoredDevice>,
}

impl JsonFileStore {
    /// Opens `path`. A missing file is an empty store; an unreadable one is
    /// logged and replaced on the first save.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let list = match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<FileContents>(&bytes) {
                Ok(c) => c.devices,
                Err(e) => {
                    warn!("Ignoring device store {}: {e}", path.display());
                    Vec::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                warn!("Cannot read device store {}: {e}", path.display());
                Vec::new()
            }
        };
        let mut map = BTreeMap::new();
        let mut order = Vec::new();
        for d in list {
            if map.insert(d.info.id.clone(), d.clone()).is_none() {
                order.push(d.info.id);
            }
        }
        Self {
            path,
            devices: Mutex::new((map, order)),
        }
    }

    fn write(&self, map: &BTreeMap<String, StoredDevice>, order: &[String]) {
        let contents = FileContents {
            devices: order.iter().filter_map(|id| map.get(id).cloned()).collect(),
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
        let guard = self.devices.lock().unwrap_or_else(|e| e.into_inner());
        let (map, order) = &*guard;
        order.iter().filter_map(|id| map.get(id).cloned()).collect()
    }

    fn save(&self, device: &StoredDevice) {
        let mut guard = self.devices.lock().unwrap_or_else(|e| e.into_inner());
        let (map, order) = &mut *guard;
        let id = &device.info.id;
        if map.get(id) == Some(device) {
            return;
        }
        if map.insert(id.clone(), device.clone()).is_none() {
            order.push(id.clone());
        }
        self.write(map, order);
    }

    fn remove(&self, id: &str) {
        let mut guard = self.devices.lock().unwrap_or_else(|e| e.into_inner());
        let (map, order) = &mut *guard;
        if map.remove(id).is_none() {
            return;
        }
        order.retain(|o| o != id);
        self.write(map, order);
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}
