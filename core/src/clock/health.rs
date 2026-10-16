//! Whether the clock is fit for media timestamping: the same model and
//! thresholds as Strom's `clock_health.rs`, so an operator gets the same
//! verdict for a phone as for a Strom host.

use crate::protocol::ClockHealthLevel;

use super::leap::EXPECTED_TAI_OFFSET_S;

/// Max error above which the clock is too unsure of the time to trust it.
pub const MAX_ERROR_LIMIT_US: i64 = 500_000;
/// Offset above which a large correction is in flight.
pub const OFFSET_LIMIT_NS: i64 = 1_000_000;

/// Where to point an operator whose clock is poorly disciplined. Strom's
/// text points at its chrony setup scripts; a phone has none, its sources
/// are the public NTP pools over whatever network it is on.
pub const DISCIPLINE_HINT: &str = "The camera disciplines its clock against public NTP pools \
     (UDP port 123). Check that the network lets NTP through and give it a few minutes; \
     with fewer than 3 agreeing sources the clock is not trusted.";

/// The inputs, named like Strom's `SystemClockInfo`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ClockInfo {
    pub synchronized: bool,
    /// `ok`, or `error` when the clock state is unusable.
    pub state: String,
    pub tai_offset_sec: i32,
    pub max_error_us: i64,
    pub offset_ns: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClockHealth {
    pub level: ClockHealthLevel,
    pub findings: Vec<String>,
}

pub fn assess_clock_health(info: &ClockInfo) -> ClockHealth {
    let mut level = ClockHealthLevel::Healthy;
    let mut findings = Vec::new();
    let bump = |to: ClockHealthLevel, level: &mut ClockHealthLevel| {
        if to > *level {
            *level = to;
        }
    };

    if !info.synchronized || info.state == "error" {
        bump(ClockHealthLevel::Bad, &mut level);
        findings.push("Clock is NOT synchronized — discipline source missing or failing.".into());
    }

    if info.tai_offset_sec == 0 {
        bump(ClockHealthLevel::Degraded, &mut level);
        findings.push(format!(
            "TAI − UTC offset is 0. Leap seconds are not configured, so TAI cannot be trusted \
             as a global time source. Expected value as of 2026 is {EXPECTED_TAI_OFFSET_S} s."
        ));
    } else if info.tai_offset_sec != EXPECTED_TAI_OFFSET_S {
        findings.push(format!(
            "TAI − UTC offset is {} s (expected {EXPECTED_TAI_OFFSET_S} as of 2026). \
             OK if your discipline source is authoritative on leap seconds.",
            info.tai_offset_sec
        ));
    }

    if info.max_error_us > MAX_ERROR_LIMIT_US {
        bump(ClockHealthLevel::Degraded, &mut level);
        findings.push(format!(
            "Max error estimate is {:.0} ms — the clock is uncertain about sync quality. \
             A healthy disciplined clock stays under 100 ms.",
            info.max_error_us as f64 / 1000.0
        ));
    }

    if info.offset_ns.abs() > OFFSET_LIMIT_NS {
        bump(ClockHealthLevel::Degraded, &mut level);
        findings.push(format!(
            "Current offset is {:.0} µs (>1 ms) — large correction in flight, sync is drifting.",
            info.offset_ns.abs() as f64 / 1000.0
        ));
    }

    if findings.is_empty() {
        findings.push("No issues detected. Clock looks well disciplined.".into());
    } else if level > ClockHealthLevel::Healthy {
        findings.push(DISCIPLINE_HINT.into());
    }

    ClockHealth { level, findings }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disciplined() -> ClockInfo {
        ClockInfo {
            synchronized: true,
            state: "ok".into(),
            tai_offset_sec: 37,
            max_error_us: 4_785,
            offset_ns: 0,
        }
    }

    #[test]
    fn well_disciplined_clock_is_healthy_without_hint() {
        let h = assess_clock_health(&disciplined());
        assert_eq!(h.level, ClockHealthLevel::Healthy);
        assert!(!h.findings.iter().any(|f| f == DISCIPLINE_HINT));
    }

    #[test]
    fn missing_leap_table_and_large_max_error_are_degraded() {
        let h = assess_clock_health(&ClockInfo {
            tai_offset_sec: 0,
            max_error_us: 940_000,
            ..disciplined()
        });
        assert_eq!(h.level, ClockHealthLevel::Degraded);
        assert!(h.findings[0].contains("TAI − UTC offset is 0"));
        assert!(h.findings[1].contains("940 ms"));
        assert_eq!(h.findings.last().unwrap(), DISCIPLINE_HINT);
    }

    #[test]
    fn thresholds_match_strom() {
        let at = |max_error_us, offset_ns| {
            assess_clock_health(&ClockInfo {
                max_error_us,
                offset_ns,
                ..disciplined()
            })
            .level
        };
        assert_eq!(at(500_000, 0), ClockHealthLevel::Healthy);
        assert_eq!(at(500_001, 0), ClockHealthLevel::Degraded);
        assert_eq!(at(0, 1_000_000), ClockHealthLevel::Healthy);
        assert_eq!(at(0, -1_000_001), ClockHealthLevel::Degraded);
    }

    #[test]
    fn unsynchronized_clock_is_bad() {
        for info in [
            ClockInfo {
                synchronized: false,
                ..disciplined()
            },
            ClockInfo {
                state: "error".into(),
                ..disciplined()
            },
        ] {
            assert_eq!(assess_clock_health(&info).level, ClockHealthLevel::Bad);
        }
    }

    #[test]
    fn unexpected_tai_offset_is_noted_but_healthy() {
        let h = assess_clock_health(&ClockInfo {
            tai_offset_sec: 36,
            ..disciplined()
        });
        assert_eq!(h.level, ClockHealthLevel::Healthy);
        assert_eq!(h.findings.len(), 1);
    }
}
