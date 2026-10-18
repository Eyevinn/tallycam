//! Shading (docs/SHADING.md): the production settings and the image
//! statistics preference. Exposure, colour and focus are plain controls:
//! the shader and the camera operator both change them, and the last write
//! wins. Protocol 1 phones know none of this and get requests as before.

use super::*;
use crate::protocol::{Production, ShutterPolicy};

/// The first protocol version with production and image stats.
pub const SHADING_PROTOCOL: u32 = 2;
/// `image_stats` per second the hub asks each phone for while a controller
/// is connected, unless a controller asks for another rate.
pub const DEFAULT_STATS_FPS: f64 = 1.0;
/// The rates a controller may ask for (the phone sends at most 4).
const STATS_FPS_RANGE: (f64, f64) = (0.2, 4.0);

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

    /// Send the stats preference, to a phone that understands it. Stats
    /// switched off are dropped, so nobody reads stale numbers.
    pub(super) fn send_stats(&mut self, controllers: usize) {
        if !self.shading() {
            return;
        }
        let cmd = self.stats_command(controllers);
        if matches!(cmd, ServerToDevice::Stats { enabled: false, .. }) {
            self.image_stats = None;
        }
        self.send(cmd);
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

/// Production settings a phone can apply: `mains_hz` 50, 60 or 0, and a
/// fixed shutter above zero.
pub(super) fn check_production(production: &Production) -> Result<(), String> {
    if !matches!(production.mains_hz, 0 | 50 | 60) {
        return Err(format!(
            "mains_hz must be 50, 60 or 0, not {}",
            production.mains_hz
        ));
    }
    if let ShutterPolicy::Fixed(0) = production.shutter {
        return Err("a fixed shutter needs a positive value".into());
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
    /// and to every controller (`production`), and kept in the store.
    /// Accepted any time: a camera that is live keeps its current settings
    /// and applies these when its stream stops. `mains_hz` is 50, 60 or 0
    /// (no mains-lit light).
    pub fn set_production(&self, production: Production) -> Result<(), String> {
        check_production(&production)?;
        let mut inner = self.lock();
        if inner.production == production {
            return Ok(());
        }
        let live = inner.devices.values().filter(|e| e.is_live()).count();
        info!(
            "Production: mains {} Hz, shutter {:?}{}",
            production.mains_hz,
            production.shutter,
            if live > 0 {
                format!(" ({live} live, applied when they stop)")
            } else {
                String::new()
            }
        );
        inner.production = production.clone();
        self.store.save_production(&production);
        for entry in inner.devices.values().filter(|e| e.shading()) {
            entry.send(ServerToDevice::Production(production.clone()));
        }
        self.emit(ControllerEvent::Production(production));
        Ok(())
    }
}
