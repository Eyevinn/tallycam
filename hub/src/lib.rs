//! TallyCam hub: the server side of the Live Contribution Protocol (LCP) v0,
//! as a library.
//!
//! The standalone `control-hub` binary runs it with the `server` feature. A
//! server that embeds it (Strom) uses the transport-independent [`Hub`] and
//! [`provision`] and either mounts the WebSocket sessions from `server` behind
//! its own routes or writes its own glue.
//!
//! - `protocol`: serde types for every LCP message.
//! - `device_key`: device public keys, ids and signatures.
//! - `hub`: device registry, controller fan-out, tally and preview gating.
//! - `provision`: the URLs and deep link handed to phones.
//! - `store`: where known devices are kept between restarts.
//! - `server` (feature `server`): Axum routes, WebSocket sessions, web UI.

pub mod device_key;
pub mod hub;
pub mod protocol;
pub mod provision;
#[cfg(feature = "server")]
pub mod server;
pub mod store;

pub use hub::{EventFrame, Hub};
pub use provision::{Provisioning, SrtTargets};
pub use store::{DeviceStore, JsonFileStore, StoredDevice};
