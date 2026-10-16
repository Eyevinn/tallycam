//! "System clock" clock source, pure part: for hosts that already run
//! chrony (or another daemon) the OS clock is used as it is, and only its
//! discipline state is reported. The platform driver reads the OS
//! (`ntp_adjtime` on Linux, `ntp_gettime` on macOS) and feeds
//! [`SystemClockReading`]s in; the mapping is a [`SoftwareClock`] that
//! follows the OS clock, so the published [`MappingSnapshot`] behaves
//! exactly like the discipline's (continuous, slewed, never stepped while
//! live).

use crate::protocol::ClockStatus;

use super::health::{assess_clock_health, ClockInfo};
use super::leap::{self, DynamicLeap};
use super::mapping::MappingSnapshot;
use super::softclock::SoftwareClock;

/// One reading of the OS clock and its kernel discipline state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SystemClockReading {
    /// Host clock when the OS clock was read.
    pub host_ns: i64,
    /// OS wall clock (UTC), ns since the Unix epoch.
    pub utc_ns: i64,
    /// Kernel synchronized flag; `None` when the OS does not tell.
    pub synchronized: Option<bool>,
    /// `ok`, `ins`, `del`, `oop`, `wait`, `error` or `unknown`.
    pub state: String,
    /// Kernel TAI - UTC (0 when the daemon did not set it).
    pub tai_offset_sec: i32,
    pub max_error_us: i64,
    pub est_error_us: i64,
    pub offset_ns: i64,
    pub frequency_ppm: f64,
}

pub struct SystemClock {
    clock: Option<SoftwareClock>,
    last: Option<SystemClockReading>,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemClock {
    pub fn new() -> Self {
        SystemClock {
            clock: None,
            last: None,
        }
    }

    fn tai_utc(&self, r: &SystemClockReading) -> i32 {
        // Trust the kernel when the daemon set it, else the built-in table.
        if r.tai_offset_sec != 0 {
            r.tai_offset_sec
        } else {
            leap::tai_utc_at_utc(r.utc_ns.div_euclid(1_000_000_000), DynamicLeap::default())
        }
    }

    /// Follows the OS clock: slews (fast while idle, bounded while live)
    /// towards each reading; steps only when off by more than 1 s and idle.
    pub fn update(&mut self, r: SystemClockReading, live: bool) {
        let tai = r.utc_ns + self.tai_utc(&r) as i64 * 1_000_000_000;
        let clock = self
            .clock
            .get_or_insert_with(|| SoftwareClock::new(r.host_ns, tai, 500e-6, 83_333.333e-6));
        clock.set_live(r.host_ns, live);
        let offset = (clock.tai_ns(r.host_ns) - tai) as f64 * 1e-9 + clock.remaining(r.host_ns);
        if offset.abs() > 1.0 && !live {
            clock.step(r.host_ns, offset);
        } else {
            clock.accumulate(r.host_ns, 0.0, offset, 0.0);
        }
        let synced = r.synchronized.unwrap_or(false) && r.state != "error";
        let snap = clock.snapshot_mut();
        snap.synced = synced;
        snap.ref_host_ns = r.host_ns;
        snap.dist_base_s = r.max_error_us as f64 * 1e-6;
        snap.dist_rate = 500e-6; // the kernel grows maxerror by 500 ppm
        snap.est_base_s = r.est_error_us as f64 * 1e-6;
        snap.est_rate = 0.0;
        self.last = Some(r);
    }

    pub fn snapshot(&self) -> Option<MappingSnapshot> {
        self.clock.as_ref().map(|c| *c.snapshot())
    }

    /// `status.clock` for this source: health from the kernel state.
    pub fn status(&self) -> ClockStatus {
        let Some(r) = &self.last else {
            return ClockStatus {
                timescale: "tai".into(),
                findings: vec!["No system clock reading yet.".into()],
                ..Default::default()
            };
        };
        let tai_utc = self.tai_utc(r);
        let mut health = assess_clock_health(&ClockInfo {
            synchronized: r.synchronized.unwrap_or(false),
            state: r.state.clone(),
            tai_offset_sec: r.tai_offset_sec,
            max_error_us: r.max_error_us,
            offset_ns: r.offset_ns,
        });
        if r.synchronized.is_none() {
            health.findings.insert(
                0,
                "The OS does not report its clock discipline state.".into(),
            );
        }
        ClockStatus {
            synced: r.synchronized.unwrap_or(false),
            health: health.level,
            findings: health.findings,
            timescale: "tai".into(),
            tai_utc_s: tai_utc,
            system_offset_ns: r.offset_ns,
            freq_ppm: r.frequency_ppm,
            est_error_us: r.est_error_us,
            max_error_us: r.max_error_us,
            leap: match r.state.as_str() {
                "ins" => "insert",
                "del" => "delete",
                "error" => "unsynchronised",
                _ => "normal",
            }
            .into(),
            extra: [("source".to_string(), serde_json::Value::from("system"))]
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_the_os_clock_and_reports_health() {
        let mut s = SystemClock::new();
        let reading = |host_ns, utc_ns| SystemClockReading {
            host_ns,
            utc_ns,
            synchronized: Some(true),
            state: "ok".into(),
            tai_offset_sec: 37,
            max_error_us: 4785,
            est_error_us: 500,
            offset_ns: 0,
            frequency_ppm: -3.0,
        };
        let utc0 = 1_791_000_000_000_000_000;
        s.update(reading(0, utc0), false);
        let snap = s.snapshot().unwrap();
        assert_eq!(snap.tai_ns(0), utc0 + 37_000_000_000);
        assert!(snap.synced);
        // The OS clock runs 50 ppm faster than the host clock: followed by slewing.
        for i in 1..=10 {
            s.update(reading(i * 1_000_000_000, utc0 + i * 1_000_050_000), true);
        }
        let snap = s.snapshot().unwrap();
        let err = snap.tai_ns(10_000_000_000) - (utc0 + 10_000_500_000 + 37_000_000_000);
        assert!(err.abs() < 100_000, "{err}");
        let st = s.status();
        assert_eq!(st.health, crate::protocol::ClockHealthLevel::Healthy);
        assert_eq!(st.tai_utc_s, 37);
    }
}
