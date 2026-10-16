//! Shading (docs/SHADING.md): who may change a camera's exposure, colour and
//! focus, the production settings, and the image statistics preference.
//!
//! The phone is the source of truth for ownership; the hub only checks a
//! controller's `set` and `command` against what the phone last reported,
//! so two shaders cannot fight without one of them taking over explicitly.
//! The phone checks again on its side. Protocol 1 phones know none of this
//! and get requests as before.

use tallycam_core::ownership::{command_group, control_group, Group};

use super::*;
use crate::protocol::{Holder, Owner, Production, ShutterPolicy};

/// The first protocol version with ownership, production and image stats.
pub const SHADING_PROTOCOL: u32 = 2;
/// `image_stats` per second the hub asks each phone for while a controller
/// is connected, unless a controller asks for another rate.
pub const DEFAULT_STATS_FPS: f64 = 1.0;
/// The rates a controller may ask for (the phone sends at most 4).
const STATS_FPS_RANGE: (f64, f64) = (0.2, 4.0);
/// Longest holder id and label, in bytes (as the phone accepts them).
const HOLDER_MAX_BYTES: usize = 64;

/// The production a hub starts with: 50 Hz mains, the shutter at the mains
/// period.
pub fn default_production() -> Production {
    Production {
        mains_hz: 50,
        shutter: ShutterPolicy::Mains,
        extra: Map::new(),
    }
}

/// Image statistics a controller asked for, per device. Applied only while
/// at least one controller is connected.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct StatsPref {
    pub(super) enabled: bool,
    pub(super) fps: f64,
}

impl Default for StatsPref {
    fn default() -> Self {
        Self {
            enabled: true,
            fps: DEFAULT_STATS_FPS,
        }
    }
}

impl DeviceEntry {
    pub(super) fn shading(&self) -> bool {
        self.protocol >= SHADING_PROTOCOL
    }

    pub(super) fn stats_command(&self, controllers: usize) -> ServerToDevice {
        let p = self.stats_pref;
        if controllers > 0 && p.enabled {
            ServerToDevice::Stats {
                enabled: true,
                fps: Some(p.fps),
            }
        } else {
            ServerToDevice::Stats {
                enabled: false,
                fps: None,
            }
        }
    }

    /// Send the stats preference, to a phone that understands it.
    pub(super) fn send_stats(&self, controllers: usize) {
        if self.shading() {
            self.send(self.stats_command(controllers));
        }
    }

    /// May `holder` change `group` on this phone? Only the remote holder of
    /// the group may, as the phone last reported it.
    fn check_group(&self, group: Group, holder: Option<&Holder>) -> Result<(), String> {
        let name = group.as_str();
        let owned = self.ownership.get(name);
        match owned {
            Some(g) if g.owner == Owner::Remote => match (&g.holder, holder) {
                (Some(owner), Some(h)) if owner.id == h.id => Ok(()),
                (Some(owner), _) => {
                    let label = if owner.label.is_empty() {
                        &owner.id
                    } else {
                        &owner.label
                    };
                    Err(format!("{name} is held by {label}: take over first"))
                }
                (None, _) => Err(format!(
                    "{name} is held by another controller: take over first"
                )),
            },
            _ => Err(format!(
                "{name} is not under remote control: take over first"
            )),
        }
    }

    /// A controller's `set` or `command` on a protocol 2 phone: every key or
    /// command in a control group must be held by the request's holder.
    /// Keys outside the groups pass. Protocol 1 phones are not checked.
    pub(super) fn check_holder(
        &self,
        msg: &ServerToDevice,
        holder: Option<&Holder>,
        shutter: ShutterPolicy,
    ) -> Result<(), String> {
        if !self.shading() {
            return Ok(());
        }
        let mut groups: Vec<Group> = match msg {
            ServerToDevice::Set { values, .. } => values
                .keys()
                .filter_map(|k| control_group(k, shutter))
                .collect(),
            ServerToDevice::Command { name, .. } => command_group(name).into_iter().collect(),
            _ => return Ok(()),
        };
        groups.sort();
        groups.dedup();
        let errors: Vec<String> = groups
            .into_iter()
            .filter_map(|g| self.check_group(g, holder).err())
            .collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    /// Is the camera streaming (or about to)?
    fn is_live(&self) -> bool {
        self.conn.is_some()
            && self
                .status
                .as_ref()
                .and_then(|s| s.stream.as_ref())
                .is_some_and(|s| matches!(s.state.as_str(), "live" | "connecting"))
    }
}

/// A holder from a controller's `own`: non-empty id, both within the
/// phone's limits.
pub(super) fn check_holder_shape(holder: &Holder) -> Result<(), String> {
    if holder.id.is_empty() {
        return Err("'holder.id' must not be empty".into());
    }
    if holder.id.len() > HOLDER_MAX_BYTES || holder.label.len() > HOLDER_MAX_BYTES {
        return Err(format!(
            "'holder' id and label are at most {HOLDER_MAX_BYTES} bytes"
        ));
    }
    Ok(())
}

/// A controller's stats rate, clamped to what the phone sends.
pub(super) fn clamp_stats_fps(fps: f64) -> f64 {
    fps.clamp(STATS_FPS_RANGE.0, STATS_FPS_RANGE.1)
}

impl Hub {
    // -----------------------------------------------------------------------
    // Production
    // -----------------------------------------------------------------------

    /// The production settings protocol 2 cameras get.
    pub fn production(&self) -> Production {
        self.lock().production.clone()
    }

    /// New production settings: sent to every connected protocol 2 camera
    /// and to every controller (`production`). Refused while any camera is
    /// live or connecting, as a phone cannot change its shutter mid-stream.
    /// `mains_hz` is 50, 60 or 0 (no mains-lit light).
    pub fn set_production(&self, production: Production) -> Result<(), String> {
        if !matches!(production.mains_hz, 0 | 50 | 60) {
            return Err(format!(
                "mains_hz must be 50, 60 or 0, not {}",
                production.mains_hz
            ));
        }
        if let ShutterPolicy::Fixed(0) = production.shutter {
            return Err("a fixed shutter needs a positive value".into());
        }
        let mut inner = self.lock();
        if inner.production == production {
            return Ok(());
        }
        if let Some(live) = inner.devices.values().find(|e| e.is_live()) {
            debug!(
                "Production change refused: {} is live",
                live.info.name.as_str()
            );
            return Err("not while cameras are live".into());
        }
        info!(
            "Production: mains {} Hz, shutter {:?}",
            production.mains_hz, production.shutter
        );
        inner.production = production.clone();
        for entry in inner.devices.values().filter(|e| e.shading()) {
            entry.send(ServerToDevice::Production(production.clone()));
        }
        self.emit(ControllerEvent::Production(production));
        Ok(())
    }
}
