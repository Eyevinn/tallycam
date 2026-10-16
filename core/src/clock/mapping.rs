//! Host clock to TAI/UTC mapping: the snapshot a clock source publishes, and
//! a seqlock cell to publish it to hot-path readers without locks or
//! allocation.
//!
//! Every clock source (the built-in discipline, or the system clock on a
//! host that already runs chrony) produces the same [`MappingSnapshot`], so
//! callers map capture timestamps identically whatever the source is.

use core::sync::atomic::{fence, AtomicU64, Ordering};

use super::leap::{self, DynamicLeap};

/// Host ns to TAI ns as a piecewise linear function, with error bounds.
///
/// `tai(h) = anchor_tai + (h - anchor_host) * (1 + rate)` where the rate is
/// `segment_freq` from the anchor to `slew_end_host_ns`, `base_freq` after
/// it and `prev_freq` before the anchor (so frames stamped just before a
/// new snapshot was published map exactly as they did with the old one).
/// The function is continuous and strictly increasing: mapped capture times
/// never jump and never run backwards.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MappingSnapshot {
    pub anchor_host_ns: i64,
    pub anchor_tai_ns: i64,
    pub prev_freq: f64,
    pub segment_freq: f64,
    pub slew_end_host_ns: i64,
    pub base_freq: f64,
    /// Correction not yet applied at the anchor, s (positive = the clock is
    /// still behind and being advanced).
    pub register_s: f64,
    /// Part of `segment_freq` that works off the register.
    pub slew_rate: f64,
    /// Synchronized to enough agreeing sources.
    pub synced: bool,
    /// Host ns the error bounds refer to (the last reference update).
    pub ref_host_ns: i64,
    /// Root distance at `ref_host_ns` (root delay / 2 + root dispersion), s.
    pub dist_base_s: f64,
    /// Growth of the root distance, s/s (skew + |residual freq| + max clock error).
    pub dist_rate: f64,
    /// Estimated error (1 sigma) at `ref_host_ns`, s.
    pub est_base_s: f64,
    pub est_rate: f64,
    /// A leap second learned from the sources, not in the built-in table.
    pub dyn_leap_utc_s: i64,
    pub dyn_leap_offset: i32,
    /// Bumped on every publish.
    pub generation: u64,
}

/// One mapped instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(C)]
pub struct Mapping {
    /// TAI ns since the PTP epoch (1970-01-01 00:00:00 TAI).
    pub tai_ns: i64,
    /// UTC ns since the Unix epoch.
    pub utc_ns: i64,
    pub synced: bool,
    /// Estimated error (1 sigma), ns.
    pub est_error_ns: i64,
    /// Maximum error bound, ns.
    pub max_error_ns: i64,
}

/// The kernel and chrony cap a maximum error at 16 s (NTP_MAX_DISPERSION).
pub const MAX_ERROR_CAP_NS: i64 = 16_000_000_000;

impl MappingSnapshot {
    /// A free-running mapping from one (host, TAI) pair, not synced.
    pub fn free_running(host_ns: i64, tai_ns: i64) -> Self {
        MappingSnapshot {
            anchor_host_ns: host_ns,
            anchor_tai_ns: tai_ns,
            slew_end_host_ns: host_ns,
            ref_host_ns: host_ns,
            dist_base_s: 16.0,
            est_base_s: 16.0,
            ..Default::default()
        }
    }

    pub fn dynamic_leap(&self) -> DynamicLeap {
        DynamicLeap {
            utc_s: self.dyn_leap_utc_s,
            offset: self.dyn_leap_offset,
        }
    }

    #[inline]
    pub fn tai_ns(&self, host_ns: i64) -> i64 {
        let d = host_ns.wrapping_sub(self.anchor_host_ns);
        if d < 0 {
            return self.anchor_tai_ns + d + (d as f64 * self.prev_freq).round() as i64;
        }
        let seg = self.slew_end_host_ns - self.anchor_host_ns;
        if d <= seg {
            return self.anchor_tai_ns + d + (d as f64 * self.segment_freq).round() as i64;
        }
        let end = self.anchor_tai_ns + seg + (seg as f64 * self.segment_freq).round() as i64;
        let rest = d - seg;
        end + rest + (rest as f64 * self.base_freq).round() as i64
    }

    #[inline]
    pub fn utc_ns(&self, host_ns: i64) -> i64 {
        leap::tai_to_utc_ns(self.tai_ns(host_ns), self.dynamic_leap())
    }

    /// Correction still to be applied at `host_ns`, s.
    #[inline]
    pub fn remaining_s(&self, host_ns: i64) -> f64 {
        let t = host_ns.clamp(
            self.anchor_host_ns,
            self.slew_end_host_ns.max(self.anchor_host_ns),
        );
        self.register_s - self.slew_rate * (t - self.anchor_host_ns) as f64 * 1e-9
    }

    /// Maximum error at `host_ns`: root distance grown since the last update,
    /// plus the correction still being slewed in. Capped at 16 s.
    #[inline]
    pub fn max_error_ns(&self, host_ns: i64) -> i64 {
        let age = ((host_ns - self.ref_host_ns) as f64 * 1e-9).abs();
        let e = self.dist_base_s + age * self.dist_rate + self.remaining_s(host_ns).abs();
        ((e * 1e9) as i64).min(MAX_ERROR_CAP_NS)
    }

    #[inline]
    pub fn est_error_ns(&self, host_ns: i64) -> i64 {
        let age = ((host_ns - self.ref_host_ns) as f64 * 1e-9).abs();
        (((self.est_base_s + age * self.est_rate) * 1e9) as i64).min(MAX_ERROR_CAP_NS)
    }

    #[inline]
    pub fn map(&self, host_ns: i64) -> Mapping {
        let tai_ns = self.tai_ns(host_ns);
        Mapping {
            tai_ns,
            utc_ns: leap::tai_to_utc_ns(tai_ns, self.dynamic_leap()),
            synced: self.synced,
            est_error_ns: self.est_error_ns(host_ns),
            max_error_ns: self.max_error_ns(host_ns),
        }
    }

    const WORDS: usize = 18;

    fn to_words(self) -> [u64; Self::WORDS] {
        [
            self.anchor_host_ns as u64,
            self.anchor_tai_ns as u64,
            self.prev_freq.to_bits(),
            self.segment_freq.to_bits(),
            self.slew_end_host_ns as u64,
            self.base_freq.to_bits(),
            self.register_s.to_bits(),
            self.slew_rate.to_bits(),
            self.synced as u64,
            self.ref_host_ns as u64,
            self.dist_base_s.to_bits(),
            self.dist_rate.to_bits(),
            self.est_base_s.to_bits(),
            self.est_rate.to_bits(),
            self.dyn_leap_utc_s as u64,
            self.dyn_leap_offset as i64 as u64,
            self.generation,
            0,
        ]
    }

    fn from_words(w: &[u64; Self::WORDS]) -> Self {
        MappingSnapshot {
            anchor_host_ns: w[0] as i64,
            anchor_tai_ns: w[1] as i64,
            prev_freq: f64::from_bits(w[2]),
            segment_freq: f64::from_bits(w[3]),
            slew_end_host_ns: w[4] as i64,
            base_freq: f64::from_bits(w[5]),
            register_s: f64::from_bits(w[6]),
            slew_rate: f64::from_bits(w[7]),
            synced: w[8] != 0,
            ref_host_ns: w[9] as i64,
            dist_base_s: f64::from_bits(w[10]),
            dist_rate: f64::from_bits(w[11]),
            est_base_s: f64::from_bits(w[12]),
            est_rate: f64::from_bits(w[13]),
            dyn_leap_utc_s: w[14] as i64,
            dyn_leap_offset: w[15] as i64 as i32,
            generation: w[16],
        }
    }
}

/// Single-writer, many-reader seqlock for a [`MappingSnapshot`]. Readers
/// never block, take no lock and do not allocate: they retry on the rare
/// overlap with a write (once per clock update, about once a minute).
pub struct SnapshotCell {
    seq: AtomicU64,
    words: [AtomicU64; MappingSnapshot::WORDS],
}

impl Default for SnapshotCell {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotCell {
    pub const fn new() -> Self {
        #[allow(clippy::declare_interior_mutable_const)]
        const ZERO: AtomicU64 = AtomicU64::new(0);
        SnapshotCell {
            seq: AtomicU64::new(0),
            words: [ZERO; MappingSnapshot::WORDS],
        }
    }

    /// Publishes a snapshot. Only one thread may write.
    pub fn store(&self, snapshot: &MappingSnapshot) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        for (cell, w) in self.words.iter().zip(snapshot.to_words()) {
            cell.store(w, Ordering::Relaxed);
        }
        self.seq.store(s.wrapping_add(2), Ordering::Release);
    }

    /// The latest snapshot; `None` before the first store.
    #[inline]
    pub fn load(&self) -> Option<MappingSnapshot> {
        loop {
            let s1 = self.seq.load(Ordering::Acquire);
            if s1 == 0 {
                return None;
            }
            if s1 & 1 == 1 {
                core::hint::spin_loop();
                continue;
            }
            let mut w = [0u64; MappingSnapshot::WORDS];
            for (out, cell) in w.iter_mut().zip(self.words.iter()) {
                *out = cell.load(Ordering::Relaxed);
            }
            fence(Ordering::Acquire);
            if self.seq.load(Ordering::Relaxed) == s1 {
                return Some(MappingSnapshot::from_words(&w));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piecewise_mapping_is_continuous() {
        let s = MappingSnapshot {
            anchor_host_ns: 1_000_000_000,
            anchor_tai_ns: 5_000_000_000,
            prev_freq: 10e-6,
            segment_freq: 510e-6,
            slew_end_host_ns: 3_000_000_000,
            base_freq: 10e-6,
            ..MappingSnapshot::free_running(0, 0)
        };
        for h in [999_999_999i64, 1_000_000_000, 1_000_000_001] {
            assert!((s.tai_ns(h) - (5_000_000_000 + (h - 1_000_000_000))).abs() <= 1);
        }
        let at_end = s.tai_ns(3_000_000_000);
        assert_eq!(at_end, 5_000_000_000 + 2_000_000_000 + 1_020_000);
        assert_eq!(s.tai_ns(3_000_000_001) - at_end, 1);
        assert_eq!(s.tai_ns(4_000_000_000) - at_end, 1_000_010_000);
    }

    #[test]
    fn seqlock_round_trip() {
        let cell = SnapshotCell::new();
        assert!(cell.load().is_none());
        let mut s = MappingSnapshot::free_running(7, 9);
        s.synced = true;
        s.dyn_leap_offset = -1;
        s.base_freq = -12.5e-6;
        cell.store(&s);
        assert_eq!(cell.load(), Some(s));
    }

    #[test]
    fn seqlock_readers_never_see_torn_snapshots() {
        use std::sync::Arc;
        let cell = Arc::new(SnapshotCell::new());
        cell.store(&MappingSnapshot::free_running(0, 0));
        let reader = {
            let cell = cell.clone();
            std::thread::spawn(move || {
                for _ in 0..200_000 {
                    let s = cell.load().unwrap();
                    // Writer keeps anchor_tai = 2 * anchor_host.
                    assert_eq!(s.anchor_tai_ns, 2 * s.anchor_host_ns);
                }
            })
        };
        for i in 0..20_000i64 {
            cell.store(&MappingSnapshot::free_running(i, 2 * i));
        }
        reader.join().unwrap();
    }
}
