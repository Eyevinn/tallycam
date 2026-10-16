//! `chronyc tracking`, `sources` and `sourcestats` style reports, and the
//! compact LCP `status.clock` object built from them.

use core::fmt::Write;
use core::net::SocketAddr;

use crate::protocol::{ClockSourceSummary, ClockStatus};

use super::engine::{LastMeasurement, LeapStatus, SourceId, SourceStatus};
use super::health::{assess_clock_health, ClockHealth, ClockInfo};

#[derive(Debug, Clone, PartialEq)]
pub struct SourceReport {
    pub id: SourceId,
    /// Pool or server name.
    pub name: String,
    pub addr: SocketAddr,
    pub status: SourceStatus,
    /// `*`, `+`, `-`, `x`, `~` or `?`.
    pub state: char,
    pub stratum: u8,
    pub poll: i8,
    pub reach: u8,
    pub last_rx_ago_s: Option<f64>,
    pub last: Option<LastMeasurement>,
    pub samples: usize,
    pub runs: usize,
    pub span_s: f64,
    /// Estimated offset now, reference minus local (raw timescale), s.
    pub est_offset_s: f64,
    pub est_offset_err_s: f64,
    /// Frequency of the raw host clock against this source, ppm (positive =
    /// host slow).
    pub freq_ppm: f64,
    pub skew_ppm: f64,
    pub std_dev_s: f64,
    pub selected: bool,
    pub sent: u64,
    pub received: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackingReport {
    pub synced: bool,
    pub ever_synced: bool,
    pub ref_addr: Option<SocketAddr>,
    pub ref_name: Option<String>,
    /// Reference id of the selected server's own reference.
    pub ref_id: Option<u32>,
    pub stratum: u8,
    pub ref_time_utc_ns: Option<i64>,
    /// Correction still being slewed in; positive = our clock fast.
    pub system_offset_s: f64,
    pub last_offset_s: f64,
    pub rms_offset_s: f64,
    /// Frequency correction of the host clock; positive = host slow.
    pub freq_ppm: f64,
    pub resid_freq_ppm: f64,
    pub skew_ppm: f64,
    pub root_delay_s: f64,
    pub root_dispersion_s: f64,
    pub update_interval_s: f64,
    pub update_age_s: Option<f64>,
    pub leap: LeapStatus,
    pub tai_utc_s: i32,
    pub est_error_s: f64,
    pub max_error_s: f64,
    pub combined: usize,
    pub live: bool,
    pub pending_step: bool,
}

impl TrackingReport {
    /// Health with Strom's model and thresholds.
    pub fn health(&self) -> ClockHealth {
        assess_clock_health(&ClockInfo {
            synchronized: self.synced,
            state: if self.ever_synced {
                "ok".into()
            } else {
                "error".into()
            },
            tai_offset_sec: self.tai_utc_s,
            max_error_us: (self.max_error_s * 1e6) as i64,
            offset_ns: (self.system_offset_s * 1e9) as i64,
        })
    }
}

fn fmt_offset(s: f64) -> String {
    let a = s.abs();
    if a >= 1.0 {
        format!("{:+.3}s", s)
    } else if a >= 1e-3 {
        format!("{:+.3}ms", s * 1e3)
    } else if a >= 1e-6 {
        format!("{:+.0}us", s * 1e6)
    } else {
        format!("{:+.0}ns", s * 1e9)
    }
}

fn fmt_ago(s: Option<f64>) -> String {
    match s {
        None => "-".into(),
        Some(s) if s < 600.0 => format!("{:.0}", s.max(0.0)),
        Some(s) if s < 36_000.0 => format!("{:.0}m", s / 60.0),
        Some(s) => format!("{:.0}h", s / 3600.0),
    }
}

/// `chronyc sources` layout.
pub fn format_sources(sources: &[SourceReport]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "MS Name/IP address                Stratum Poll Reach LastRx Last sample"
    );
    let _ = writeln!(
        out,
        "==============================================================================="
    );
    for s in sources {
        let last = match s.last {
            Some(l) => format!(
                "{:>9} +/- {:>8}",
                fmt_offset(l.offset),
                fmt_offset(l.error).trim_start_matches('+')
            ),
            None => String::new(),
        };
        let _ = writeln!(
            out,
            "^{} {:<32} {:>2} {:>4} {:>5o} {:>6} {}",
            s.state,
            s.addr.ip().to_string(),
            s.stratum,
            s.poll,
            s.reach,
            fmt_ago(s.last_rx_ago_s),
            last
        );
    }
    out
}

/// `chronyc sourcestats` layout.
pub fn format_sourcestats(sources: &[SourceReport]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Name/IP Address            NP  NR  Span  Frequency  Freq Skew  Offset  Std Dev"
    );
    let _ = writeln!(
        out,
        "=============================================================================="
    );
    for s in sources {
        let _ = writeln!(
            out,
            "{:<25} {:>3} {:>3} {:>5} {:>10.3} {:>10.3} {:>7} {:>8}",
            s.addr.ip().to_string(),
            s.samples,
            s.runs,
            format!("{:.0}", s.span_s),
            s.freq_ppm,
            s.skew_ppm,
            fmt_offset(s.est_offset_s),
            fmt_offset(s.std_dev_s).trim_start_matches('+'),
        );
    }
    out
}

/// `chronyc tracking` layout, plus the error bounds and health.
pub fn format_tracking(t: &TrackingReport) -> String {
    let mut out = String::new();
    let refname = match (&t.ref_addr, &t.ref_name) {
        (Some(a), Some(n)) => format!("{} ({})", a.ip(), n),
        _ => "-".into(),
    };
    let fast_slow = |v: f64, pos: &str, neg: &str| {
        if v >= 0.0 {
            pos.to_string()
        } else {
            neg.to_string()
        }
    };
    let _ = writeln!(out, "Reference ID    : {refname}");
    let _ = writeln!(out, "Stratum         : {}", t.stratum);
    let _ = writeln!(
        out,
        "Ref time (UTC)  : {}",
        t.ref_time_utc_ns
            .map_or("-".into(), |ns| super::leap::format_utc(
                ns.div_euclid(1_000_000_000)
            ))
    );
    let _ = writeln!(
        out,
        "System time     : {:.9} seconds {} of NTP time",
        t.system_offset_s.abs(),
        fast_slow(t.system_offset_s, "fast", "slow")
    );
    let _ = writeln!(out, "Last offset     : {:+.9} seconds", t.last_offset_s);
    let _ = writeln!(out, "RMS offset      : {:.9} seconds", t.rms_offset_s);
    let _ = writeln!(
        out,
        "Frequency       : {:.3} ppm {}",
        t.freq_ppm.abs(),
        fast_slow(t.freq_ppm, "slow", "fast")
    );
    let _ = writeln!(out, "Residual freq   : {:+.3} ppm", t.resid_freq_ppm);
    let _ = writeln!(out, "Skew            : {:.3} ppm", t.skew_ppm);
    let _ = writeln!(out, "Root delay      : {:.9} seconds", t.root_delay_s);
    let _ = writeln!(out, "Root dispersion : {:.9} seconds", t.root_dispersion_s);
    let _ = writeln!(out, "Update interval : {:.1} seconds", t.update_interval_s);
    let _ = writeln!(
        out,
        "Leap status     : {}",
        match t.leap {
            LeapStatus::Normal => "Normal",
            LeapStatus::Insert => "Insert second",
            LeapStatus::Delete => "Delete second",
            LeapStatus::Unsynchronised => "Not synchronised",
        }
    );
    let _ = writeln!(out, "TAI - UTC       : {} s", t.tai_utc_s);
    let _ = writeln!(out, "Est. error      : {:.6} seconds", t.est_error_s);
    let _ = writeln!(out, "Max. error      : {:.6} seconds", t.max_error_s);
    let h = t.health();
    let _ = writeln!(
        out,
        "Synchronised    : {}   Health: {:?}",
        if t.synced { "yes" } else { "no" },
        h.level
    );
    out
}

/// The LCP `status.clock` object. `device_offset_ms`: disciplined UTC minus
/// the platform wall clock, when the caller can read one.
pub fn clock_status(
    t: &TrackingReport,
    sources: &[SourceReport],
    device_offset_ms: Option<f64>,
) -> ClockStatus {
    let health = t.health();
    let us = |s: f64| (s * 1e6).round() as i64;
    ClockStatus {
        synced: t.synced,
        health: health.level,
        findings: health.findings,
        timescale: "tai".into(),
        tai_utc_s: t.tai_utc_s,
        reference: t.ref_addr.map(|a| a.ip().to_string()),
        reference_name: t.ref_name.clone(),
        stratum: t.stratum,
        update_age_s: t.update_age_s.map(|a| (a * 10.0).round() / 10.0),
        system_offset_ns: (t.system_offset_s * 1e9).round() as i64,
        last_offset_ns: (t.last_offset_s * 1e9).round() as i64,
        rms_offset_ns: (t.rms_offset_s * 1e9).round() as i64,
        freq_ppm: (t.freq_ppm * 1000.0).round() / 1000.0,
        resid_freq_ppm: (t.resid_freq_ppm * 1000.0).round() / 1000.0,
        skew_ppm: (t.skew_ppm * 1000.0).round() / 1000.0,
        root_delay_us: us(t.root_delay_s),
        root_dispersion_us: us(t.root_dispersion_s),
        update_interval_s: (t.update_interval_s * 10.0).round() / 10.0,
        leap: t.leap.name().into(),
        est_error_us: us(t.est_error_s),
        max_error_us: us(t.max_error_s),
        device_offset_ms: device_offset_ms.map(|v| (v * 1000.0).round() / 1000.0),
        selected: sources.iter().filter(|s| s.state == '*').count() as u32,
        combined: sources.iter().filter(|s| s.state == '+').count() as u32,
        reachable: sources.iter().filter(|s| s.reach != 0).count() as u32,
        total: sources.len() as u32,
        sources: sources
            .iter()
            .map(|s| ClockSourceSummary {
                state: s.state.to_string(),
                name: s.name.clone(),
                addr: s.addr.ip().to_string(),
                stratum: s.stratum,
                poll: s.poll,
                reach: s.reach,
                last_rx_s: s.last_rx_ago_s.map(|a| a.max(0.0) as u64),
                offset_us: s.last.map(|l| us(l.offset)),
                error_us: s.last.map(|l| us(l.error)),
                extra: Default::default(),
            })
            .collect(),
        extra: Default::default(),
    }
}

/// `status.clock` as compact JSON.
pub fn to_json(status: &ClockStatus) -> String {
    serde_json::to_string(status).unwrap_or_default()
}
