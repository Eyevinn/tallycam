//! Live Contribution Protocol (LCP) v0 message types.
//!
//! The device protocol (phone <-> server) comes from the shared core
//! (`tallycam_core::protocol`) and is re-exported here. This module adds
//! the hub-specific controller envelope (operator UI <-> hub,
//! `/lcp/v0/control`) around the device messages.
//!
//! Unknown `type` values and unknown fields are ignored. Fields the hub does not
//! interpret are kept in `extra` maps so they reach the operator UI unchanged.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use tallycam_core::protocol::*;

// ---------------------------------------------------------------------------
// Controller (operator UI) <-> hub
// ---------------------------------------------------------------------------

/// A controller message: a [`ServerToDevice`] message plus `"device": "<id>"`.
/// The hub removes `device` and forwards the rest to that phone.
#[derive(Debug, Clone, PartialEq)]
pub struct ControllerRequest {
    pub device: String,
    pub message: ServerToDevice,
}

impl ControllerRequest {
    /// Parse a controller frame. Returns `Ok(None)` for messages to ignore
    /// (unknown `type`), `Err` for malformed known messages.
    pub fn parse(text: &str) -> Result<Option<Self>, String> {
        let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        Self::from_value(value)
    }

    fn from_value(mut value: Value) -> Result<Option<Self>, String> {
        let obj = value.as_object_mut().ok_or("expected a JSON object")?;
        let ty = obj
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !matches!(
            ty.as_str(),
            "set" | "command" | "tally" | "message" | "preview" | "stats"
        ) {
            return Ok(None);
        }
        let device = match obj.remove("device") {
            Some(Value::String(d)) => d,
            _ => return Err(format!("'{ty}' needs a 'device' string")),
        };
        // An older controller may still send a `holder` (from when the hub
        // had ownership): ignored.
        obj.remove("holder");
        let message = serde_json::from_value(value).map_err(|e| e.to_string())?;
        Ok(Some(Self { device, message }))
    }
}

/// Everything a controller can ask of the hub.
#[derive(Debug, Clone, PartialEq)]
pub enum ControllerInput {
    /// For one device ([`ControllerRequest`]).
    Device(ControllerRequest),
    /// `{"type":"production","mains_hz":50,"shutter":"mains"}`: new
    /// production settings for every camera (no `device`).
    Production(Production),
    /// `{"type":"watch","devices":["id",...]}`: the devices whose previews
    /// this controller shows. `null` (or no `devices`) is every device, as
    /// for a controller that never sends `watch`.
    Watch(Option<Vec<String>>),
    /// `{"type":"tags","device":"id","tags":["Studio A"]}`: the device's tags,
    /// replacing the old ones (normalized by the hub).
    Tags { device: String, tags: Vec<String> },
    /// `{"type":"arrange","order":["id",...]}`: the display order of the
    /// devices.
    Arrange(Vec<String>),
}

impl ControllerInput {
    /// Parse a controller frame. Returns `Ok(None)` for messages to ignore
    /// (unknown `type`), `Err` for malformed known messages.
    pub fn parse(text: &str) -> Result<Option<Self>, String> {
        let mut value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if value.get("type").and_then(Value::as_str) == Some("production") {
            let obj = value.as_object_mut().ok_or("expected a JSON object")?;
            obj.remove("type");
            obj.remove("id");
            let production = serde_json::from_value(value).map_err(|e| e.to_string())?;
            return Ok(Some(Self::Production(production)));
        }
        match value.get("type").and_then(Value::as_str) {
            Some("watch") => {
                return match value.get("devices") {
                    None | Some(Value::Null) => Ok(Some(Self::Watch(None))),
                    Some(list) => Ok(Some(Self::Watch(Some(strings(list, "watch", "devices")?)))),
                };
            }
            Some("tags") => {
                let device = match value.get("device") {
                    Some(Value::String(d)) => d.clone(),
                    _ => return Err("'tags' needs a 'device' string".into()),
                };
                let tags = strings(value.get("tags").unwrap_or(&Value::Null), "tags", "tags")?;
                return Ok(Some(Self::Tags { device, tags }));
            }
            Some("arrange") => {
                let order = strings(
                    value.get("order").unwrap_or(&Value::Null),
                    "arrange",
                    "order",
                )?;
                return Ok(Some(Self::Arrange(order)));
            }
            _ => {}
        }
        Ok(ControllerRequest::from_value(value)?.map(Self::Device))
    }
}

/// `value` as a list of strings: field `field` of a `ty` message.
fn strings(value: &Value, ty: &str, field: &str) -> Result<Vec<String>, String> {
    let err = || format!("'{ty}' needs '{field}' as a list of strings");
    value
        .as_array()
        .ok_or_else(err)?
        .iter()
        .map(|v| v.as_str().map(str::to_string).ok_or_else(err))
        .collect()
}

/// Everything the hub knows about one device, sent in `snapshot`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceSnapshot {
    pub id: String,
    pub info: DeviceInfo,
    pub controls: Vec<ControlDescriptor>,
    pub commands: Vec<CommandDescriptor>,
    pub state: Values,
    pub status: Option<Status>,
    /// Latest preview frame, so a new controller has a picture right away.
    pub preview: Option<Preview>,
    pub tally: TallyState,
    /// The LCP version from the device's `hello`. Production and image
    /// statistics need 2.
    #[serde(default)]
    pub protocol: u32,
    /// The latest `image_stats` (protocol 2, only while a controller asks
    /// and the device is online).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_stats: Option<ImageStats>,
    pub online: bool,
    /// SHA-256 of the device's public key (hex), for a device that proved a
    /// key; absent for one admitted without (an app before device keys).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_fingerprint: Option<String>,
    /// The operator's tags (`tags`), in their order; absent when none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// When the device last said hello or went offline (unix seconds);
    /// absent for a device never seen since this was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_s: Option<u64>,
    /// This camera's own production settings, when the embedding server set
    /// them ([`crate::Hub::set_device_production`], e.g. its flow's); absent:
    /// the hub's `production`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub production: Option<Production>,
}

/// Hub -> controller. Device messages are forwarded as `device_<type>` with
/// the device id in `device` and the original fields next to it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControllerEvent {
    Snapshot {
        protocol: u32,
        devices: Vec<DeviceSnapshot>,
        /// The production settings every protocol 2 camera gets.
        production: Production,
    },
    /// The production settings changed (by any controller, or the embedding
    /// server).
    Production(Production),
    /// A device connected or resent `hello`. `info` carries the hello `device`
    /// object (renamed because `device` holds the id here).
    DeviceHello {
        device: String,
        info: DeviceInfo,
        controls: Vec<ControlDescriptor>,
        commands: Vec<CommandDescriptor>,
        /// As in [`DeviceSnapshot::protocol`].
        #[serde(default)]
        protocol: u32,
        /// As in [`DeviceSnapshot::key_fingerprint`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_fingerprint: Option<String>,
    },
    DeviceState {
        device: String,
        values: Values,
    },
    DeviceStatus {
        device: String,
        #[serde(flatten)]
        status: Status,
    },
    DevicePreview {
        device: String,
        #[serde(flatten)]
        preview: Preview,
    },
    DeviceResult {
        device: String,
        #[serde(flatten)]
        result: CommandResult,
    },
    DeviceLog {
        device: String,
        #[serde(flatten)]
        log: Log,
    },
    /// A camera got its own production settings, or (`production` null)
    /// follows the hub's again.
    DeviceProduction {
        device: String,
        production: Option<Production>,
    },
    /// The device's `image_stats`, forwarded.
    DeviceImageStats {
        device: String,
        #[serde(flatten)]
        stats: ImageStats,
    },
    DeviceOffline {
        device: String,
        /// When it went offline (unix seconds).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_seen_s: Option<u64>,
    },
    /// The device was forgotten (revoked): drop it from the list.
    DeviceRemoved {
        device: String,
    },
    /// Tally changed (by any controller), so every controller stays in sync.
    DeviceTally {
        device: String,
        state: TallyState,
    },
    /// A device's tags changed (`tags`), normalized.
    DeviceTags {
        device: String,
        tags: Vec<String>,
    },
    /// The display order changed (`arrange`): every device id, in order.
    Order {
        devices: Vec<String>,
    },
    /// Reply to the sending controller only, e.g. unknown or offline device.
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device: Option<String>,
        message: String,
    },
}
