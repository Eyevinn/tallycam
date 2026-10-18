//! Swift bindings of the TallyCam core.
//!
//! - UniFFI: [`ClockService`], which runs a clock source (the built-in
//!   discipline or the system clock) on its own thread, plus its reports.
//! - UniFFI: [`SrtCaller`], the SRT caller (tallycam-srt), with its events
//!   on a foreign [`SrtListener`].
//! - UniFFI: device keys (`device_key`): the hello fields from the public
//!   key, the challenge payload the Secure Enclave signs, the `auth` message.
//! - UniFFI: camera logic (`camera`): the mains frequency per region and the
//!   bitrate defaults.
//! - UniFFI: image statistics (`image_stats`) of a sampled picture.
//! - C ABI (`include/tallycam_core_hot.h`): the per-frame calls: the clock
//!   mapping, the SEI writer, the MPEG-TS muxer (which queues straight onto
//!   the SRT caller; `lc_srt_send` sends a ready payload the same way), the
//!   genlock and auto ISO loops. The clock calls read the
//!   snapshot the running service publishes through a seqlock; the SRT send
//!   copies into a lock-free queue: no locks, no allocation, no UniFFI
//!   buffers per frame.

use std::sync::{Arc, OnceLock};

use tallycam_core::clock::driver::{default_host_clock, HostClock};
use tallycam_core::clock::SnapshotCell;

uniffi::setup_scaffolding!();

// One module per area; the C declarations are in include/tallycam_core_hot.h.
mod camera;
mod clock;
mod device_key;
mod genlock;
mod image_stats;
mod sei;
mod srt;
mod ts;

// ---------------------------------------------------------------------------
// Process-wide host clock and published mapping
// ---------------------------------------------------------------------------

#[cfg(target_vendor = "apple")]
pub(crate) fn mach() -> &'static tallycam_core::clock::driver::MachHostClock {
    static MACH: OnceLock<tallycam_core::clock::driver::MachHostClock> = OnceLock::new();
    MACH.get_or_init(Default::default)
}

pub(crate) fn host() -> &'static Arc<dyn HostClock> {
    static HOST: OnceLock<Arc<dyn HostClock>> = OnceLock::new();
    HOST.get_or_init(default_host_clock)
}

/// The mapping the running clock service publishes; shared by all services
/// (only one runs at a time).
pub(crate) fn cell() -> &'static Arc<SnapshotCell> {
    static CELL: OnceLock<Arc<SnapshotCell>> = OnceLock::new();
    CELL.get_or_init(|| Arc::new(SnapshotCell::new()))
}
