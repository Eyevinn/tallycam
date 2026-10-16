//! The clock: the C ABI that maps capture host times to TAI on every frame,
//! and the UniFFI clock service that runs the discipline and publishes the
//! mapping.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tallycam_core::clock::driver::{
    DisciplineDriver, DriverOptions, FileLogSink, LogSink, StdResolver, SystemClockDriver,
};
use tallycam_core::clock::leap::{self, DynamicLeap};
use tallycam_core::clock::log::LogKind;
use tallycam_core::clock::{ClockConfig, ClockSource, PathInfo};

#[cfg(target_vendor = "apple")]
use crate::mach;
use crate::{cell, host};

#[repr(C)]
pub struct LcClockMapping {
    pub tai_ns: i64,
    pub utc_ns: i64,
    pub est_error_ns: i64,
    pub max_error_ns: i64,
    pub synced: bool,
    pub disciplined: bool,
}

#[no_mangle]
pub extern "C" fn lc_host_now_ns() -> i64 {
    host().now_ns()
}

#[no_mangle]
pub extern "C" fn lc_host_ticks_to_ns(ticks: u64) -> i64 {
    #[cfg(target_vendor = "apple")]
    {
        mach().ticks_to_ns(ticks)
    }
    #[cfg(not(target_vendor = "apple"))]
    {
        ticks as i64
    }
}

/// # Safety
/// `out` must point to a writable `LcClockMapping`.
#[no_mangle]
pub unsafe extern "C" fn lc_clock_map(host_ns: i64, out: *mut LcClockMapping) {
    if out.is_null() {
        return;
    }
    let m = match cell().load() {
        Some(snap) => {
            let m = snap.map(host_ns);
            LcClockMapping {
                tai_ns: m.tai_ns,
                utc_ns: m.utc_ns,
                est_error_ns: m.est_error_ns,
                max_error_ns: m.max_error_ns,
                synced: m.synced,
                disciplined: true,
            }
        }
        None => {
            // No service yet: the device wall clock, relative to now.
            let h = host();
            let utc = h.wall_utc_ns() - (h.now_ns() - host_ns);
            LcClockMapping {
                tai_ns: leap::utc_to_tai_ns(utc, DynamicLeap::default()),
                utc_ns: utc,
                est_error_ns: -1,
                max_error_ns: -1,
                synced: false,
                disciplined: false,
            }
        }
    };
    // SAFETY: checked non-null; the caller guarantees it is writable.
    unsafe { out.write(m) };
}

// ---------------------------------------------------------------------------
// UniFFI: clock service
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ClockMode {
    /// The built-in chrony-like NTP discipline.
    Discipline,
    /// The OS clock, for hosts that run chrony themselves.
    System,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ClockOptions {
    pub mode: ClockMode,
    /// chrony.conf text with `pool`/`server` lines; `None` = Strom's config.
    pub chrony_conf: Option<String>,
    /// Append the measurements/statistics/tracking/selection log here.
    pub log_file: Option<String>,
    /// chrony-style driftfile, so a restart starts from the known frequency.
    pub drift_file: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct NetworkPath {
    pub satisfied: bool,
    pub ipv4: bool,
    pub ipv6: bool,
    /// Identity of the path, e.g. "wifi:en0" or "cellular:pdp_ip0".
    pub key: String,
}

#[derive(Debug, uniffi::Error)]
pub enum ClockError {
    Config { message: String },
}

impl std::fmt::Display for ClockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClockError::Config { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ClockError {}

#[derive(uniffi::Object)]
pub struct ClockService {
    source: Mutex<Option<Arc<dyn ClockSource>>>,
    discipline: Option<Arc<DisciplineDriver>>,
    system: Option<Arc<SystemClockDriver>>,
}

#[uniffi::export]
impl ClockService {
    /// Starts a clock source on its own thread. It publishes the mapping the
    /// C hot path (`lc_clock_map`) reads.
    #[uniffi::constructor]
    pub fn start(options: ClockOptions) -> Result<Arc<Self>, ClockError> {
        match options.mode {
            ClockMode::Discipline => {
                let cfg = match &options.chrony_conf {
                    Some(text) => ClockConfig::from_chrony_conf(text)
                        .map_err(|message| ClockError::Config { message })?,
                    None => ClockConfig::default(),
                };
                let log_sink = options.log_file.as_ref().map(|p| {
                    Box::new(FileLogSink::new(PathBuf::from(p), 4 << 20)) as Box<dyn LogSink>
                });
                let driver = DisciplineDriver::start(
                    cfg,
                    host().clone(),
                    Arc::new(StdResolver),
                    DriverOptions {
                        log_sink,
                        drift_file: options.drift_file.map(PathBuf::from),
                    },
                    cell().clone(),
                );
                let source: Arc<dyn ClockSource> = driver.clone();
                Ok(Arc::new(ClockService {
                    source: Mutex::new(Some(source)),
                    discipline: Some(driver),
                    system: None,
                }))
            }
            ClockMode::System => {
                let driver = SystemClockDriver::start(host().clone(), cell().clone());
                let source: Arc<dyn ClockSource> = driver.clone();
                Ok(Arc::new(ClockService {
                    source: Mutex::new(Some(source)),
                    discipline: None,
                    system: Some(driver),
                }))
            }
        }
    }

    /// Stops the thread (and saves the driftfile). The last mapping stays
    /// published and free runs.
    pub fn stop(&self) {
        self.source.lock().unwrap().take();
        if let Some(d) = &self.discipline {
            d.stop();
        }
        if let Some(s) = &self.system {
            s.stop();
        }
    }

    /// While live: never step, slew at most 500 ppm.
    pub fn set_live(&self, live: bool) {
        if let Some(s) = self.source.lock().unwrap().as_ref() {
            s.set_live(live);
        }
    }

    pub fn path_changed(&self, path: NetworkPath) {
        if let Some(s) = self.source.lock().unwrap().as_ref() {
            s.path_changed(PathInfo {
                satisfied: path.satisfied,
                ipv4: path.ipv4,
                ipv6: path.ipv6,
                key: path.key,
            });
        }
    }

    /// The LCP `status.clock` object as JSON.
    pub fn status_json(&self) -> String {
        match self.source.lock().unwrap().as_ref() {
            Some(s) => serde_json::to_string(&s.status()).unwrap_or_default(),
            None => "{\"synced\":false}".into(),
        }
    }

    /// chronyc-like `sources`, `sourcestats` and `tracking` text.
    pub fn reports_text(&self) -> String {
        self.source
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.reports_text())
            .unwrap_or_default()
    }

    /// Newest log lines of the given kinds (`measurements`, `statistics`,
    /// `tracking`, `selection`, `event`; all when empty), oldest first.
    pub fn log_lines(&self, kinds: Vec<String>, limit: u32) -> Vec<String> {
        let kinds: Vec<LogKind> = kinds.iter().filter_map(|k| LogKind::parse(k)).collect();
        self.source
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.log_tail(&kinds, limit as usize))
            .unwrap_or_default()
    }
}
