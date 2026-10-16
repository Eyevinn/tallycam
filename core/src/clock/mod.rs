//! Clock: maps the host clock (the clock media capture timestamps are in) to
//! TAI and UTC, with a synced flag, error bounds and a health verdict.
//!
//! Two clock sources publish the same [`mapping::MappingSnapshot`]:
//!
//! - the built-in discipline ([`engine::Engine`]), an NTP client that
//!   behaves like chrony configured as on Strom hosts (`pool ... iburst
//!   minpoll 6 maxpoll 8`, `minsources 3`, `makestep 1.0 3`,
//!   `leapsectz right/UTC`, `log measurements statistics tracking
//!   selection`), for devices that cannot (or must not) touch the system
//!   clock: phones, browsers, locked-down hosts;
//! - the system clock ([`system::SystemClock`]) for hosts that already run
//!   chrony: the OS clock is used as it is, only its health is reported.
//!
//! Everything here is pure: host time is passed in, network I/O, DNS,
//! timers and storage belong to the platform driver (the `driver` feature
//! has one for std platforms). It builds for `wasm32-unknown-unknown`.

pub mod config;
pub mod engine;
pub mod health;
pub mod leap;
pub mod log;
pub mod mapping;
pub mod ntp;
pub mod regress;
pub mod report;
pub mod sim;
pub mod softclock;
pub mod sourcestats;
pub mod system;

#[cfg(feature = "driver")]
pub mod driver;

use crate::protocol::ClockStatus;

/// Nanoseconds as seconds.
pub(crate) fn secs(ns: i64) -> f64 {
    ns as f64 * 1e-9
}

pub use config::{ClockConfig, SourceConfig, SourceKind};
pub use engine::{Action, Engine, PathInfo, RandomSource, SourceId};
pub use mapping::{Mapping, MappingSnapshot, SnapshotCell};

/// A running clock source as callers see it, whatever its kind. The hot
/// path is [`ClockSource::map`]: lock free, no allocation.
pub trait ClockSource: Send + Sync {
    /// Latest published mapping; `None` before the source has one.
    fn snapshot(&self) -> Option<MappingSnapshot>;

    /// Host ns to TAI/UTC with error bounds. Unsynchronized (with a
    /// best-effort time) until the source has a snapshot.
    fn map(&self, host_ns: i64) -> Option<Mapping> {
        self.snapshot().map(|s| s.map(host_ns))
    }

    /// The `status.clock` object (tracking, health, sources).
    fn status(&self) -> ClockStatus;

    /// chrony-style text reports: tracking, sources and sourcestats.
    fn reports_text(&self) -> String;

    /// The newest log lines of the given kinds (all when empty).
    fn log_tail(&self, kinds: &[log::LogKind], limit: usize) -> Vec<String>;

    /// While live the mapping is never stepped and slews at a bounded rate.
    fn set_live(&self, live: bool);

    /// The network path changed (ignored by sources that do not care).
    fn path_changed(&self, path: PathInfo);
}
