//! The software clock the discipline steers: TAI as a function of the host
//! clock. The platform clock is never touched.
//!
//! Corrections are slewed like chrony's generic driver (`sys_generic.c`): an
//! offset goes into a register that is worked off by a temporary frequency
//! offset, at most `max_slew` (500 ppm while live, chrony's default 83333 ppm
//! otherwise), so the mapped time stays continuous and frame intervals change
//! by at most that fraction. A step is the exception and only happens
//! through [`SoftwareClock::step`], which the engine allows only while the
//! stream is not live.

use super::mapping::MappingSnapshot;

const MIN_SLEW_DURATION: f64 = 1.0;
const MAX_SLEW_DURATION: f64 = 1.0e4;
/// Frequency corrections are kept within this (chrony's default maxdrift is
/// larger; a phone oscillator is within +-100 ppm).
const MAX_FREQ: f64 = 5000e-6;

#[derive(Debug, Clone)]
pub struct SoftwareClock {
    snap: MappingSnapshot,
    max_slew_live: f64,
    max_slew_idle: f64,
    live: bool,
    /// From the last reference update, for re-planning a slew.
    correction_rate: f64,
}

fn secs(ns: i64) -> f64 {
    ns as f64 * 1e-9
}

impl SoftwareClock {
    /// Starts free running at `tai_ns` (from the platform wall clock).
    pub fn new(host_ns: i64, tai_ns: i64, max_slew_live: f64, max_slew_idle: f64) -> Self {
        SoftwareClock {
            snap: MappingSnapshot::free_running(host_ns, tai_ns),
            max_slew_live,
            max_slew_idle,
            live: false,
            correction_rate: 0.0,
        }
    }

    pub fn snapshot(&self) -> &MappingSnapshot {
        &self.snap
    }

    pub fn snapshot_mut(&mut self) -> &mut MappingSnapshot {
        &mut self.snap
    }

    pub fn tai_ns(&self, host_ns: i64) -> i64 {
        self.snap.tai_ns(host_ns)
    }

    /// Correction still to be applied, s (positive = clock behind).
    pub fn remaining(&self, host_ns: i64) -> f64 {
        self.snap.remaining_s(host_ns)
    }

    /// Base frequency: the clock runs at `1 + base_freq` host seconds per
    /// second; positive = the host oscillator is slow.
    pub fn base_freq(&self) -> f64 {
        self.snap.base_freq
    }

    pub fn is_live(&self) -> bool {
        self.live
    }

    pub fn max_slew(&self) -> f64 {
        if self.live {
            self.max_slew_live
        } else {
            self.max_slew_idle
        }
    }

    /// When the current slew ends (the engine re-plans then if a remainder is
    /// left).
    pub fn slew_end(&self) -> i64 {
        self.snap.slew_end_host_ns
    }

    /// Re-anchors at `now` with a new base frequency and register; the mapped
    /// time is continuous at `now`.
    fn reanchor(&mut self, now: i64, base_freq: f64, register: f64) {
        let tai_now = self.snap.tai_ns(now);
        let prev_freq = self.current_freq(now);
        self.snap.anchor_host_ns = now;
        self.snap.anchor_tai_ns = tai_now;
        self.snap.prev_freq = prev_freq;
        self.snap.base_freq = base_freq.clamp(-MAX_FREQ, MAX_FREQ);
        self.snap.register_s = register;
        self.plan_slew(now);
    }

    /// Total frequency in effect at `now`.
    fn current_freq(&self, now: i64) -> f64 {
        if now < self.snap.anchor_host_ns {
            self.snap.prev_freq
        } else if now <= self.snap.slew_end_host_ns {
            self.snap.segment_freq
        } else {
            self.snap.base_freq
        }
    }

    /// chrony's `update_slew`: the duration follows from the correction
    /// rate, the slew frequency is bounded by `max_slew`.
    fn plan_slew(&mut self, now: i64) {
        let reg = self.snap.register_s;
        let max = self.max_slew();
        let mut duration = if reg != 0.0 {
            (self.correction_rate / reg.abs()).clamp(MIN_SLEW_DURATION, MAX_SLEW_DURATION)
        } else {
            MAX_SLEW_DURATION
        };
        let corr = (reg / duration).clamp(-max, max);
        duration = if corr != 0.0 {
            (reg / corr).clamp(MIN_SLEW_DURATION, MAX_SLEW_DURATION)
        } else {
            0.0
        };
        self.snap.slew_rate = corr;
        self.snap.segment_freq = self.snap.base_freq + corr;
        self.snap.slew_end_host_ns = now + (duration * 1e9) as i64;
    }

    /// Applies a reference update (chrony `LCL_AccumulateFrequencyAndOffset`):
    /// `dfreq` and `offset` in chrony's sign (positive = clock fast).
    pub fn accumulate(&mut self, now: i64, dfreq: f64, offset: f64, correction_rate: f64) {
        self.correction_rate = correction_rate;
        let register = self.remaining(now) - offset;
        let base = self.snap.base_freq - dfreq;
        self.reanchor(now, base, register);
    }

    /// Steps the clock by `-offset` (offset positive = clock fast) and clears
    /// the register. The mapping jumps: only when nothing is live.
    pub fn step(&mut self, now: i64, offset: f64) {
        let target =
            self.snap.tai_ns(now) + (self.remaining(now) * 1e9) as i64 - (offset * 1e9) as i64;
        let base = self.snap.base_freq;
        self.snap.anchor_host_ns = now;
        self.snap.anchor_tai_ns = target;
        self.snap.prev_freq = base;
        self.snap.register_s = 0.0;
        self.plan_slew(now);
    }

    /// Sets the base frequency (from a saved drift), continuous at `now`.
    pub fn set_base_freq(&mut self, now: i64, freq: f64) {
        let reg = self.remaining(now);
        self.reanchor(now, freq, reg);
    }

    /// Continues a slew whose planned duration ran out with a remainder.
    pub fn reslew(&mut self, now: i64) {
        let reg = self.remaining(now);
        let base = self.snap.base_freq;
        self.reanchor(now, base, reg);
    }

    /// Live streams get the bounded slew rate from now on.
    pub fn set_live(&mut self, now: i64, live: bool) {
        if self.live != live {
            self.live = live;
            self.reslew(now);
        }
    }

    /// The host clock stood still for `delta_ns` (device sleep): the true
    /// time moved on by that much, so the mapping does too.
    pub fn host_paused(&mut self, now: i64, delta_ns: i64) {
        let reg = self.remaining(now);
        let base = self.snap.base_freq;
        self.reanchor(now, base, reg);
        self.snap.anchor_tai_ns += delta_ns;
    }

    /// Seconds since the anchor, for tests and reports.
    pub fn since_anchor(&self, now: i64) -> f64 {
        secs(now - self.snap.anchor_host_ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: i64 = 1_000_000_000;

    #[test]
    fn slew_is_bounded_and_monotonic() {
        let mut c = SoftwareClock::new(0, 1_000 * S, 500e-6, 83_333e-6);
        c.set_live(0, true);
        // 10 ms behind, frequency unknown so far.
        c.accumulate(S, 0.0, -0.010, 0.0);
        let mut last = c.tai_ns(S);
        let frame = S / 25;
        let mut h = S;
        while h < 40 * S {
            h += frame;
            let t = c.tai_ns(h);
            let interval = t - last;
            assert!(interval > 0);
            assert!(
                (interval - frame).abs() <= frame / 2000 + 1,
                "interval {interval}"
            );
            last = t;
        }
        // 10 ms at 500 ppm takes 20 s; done after 40 s.
        assert!(c.remaining(40 * S).abs() < 1e-9);
        assert_eq!(c.tai_ns(40 * S), 1_000 * S + 40 * S + 10_000_000);
    }

    #[test]
    fn idle_slews_fast_and_live_switch_is_continuous() {
        let mut c = SoftwareClock::new(0, 0, 500e-6, 83_333e-6);
        c.accumulate(0, 0.0, -0.5, 0.0);
        // 0.5 s at 83333 ppm takes 6 s.
        assert!(c.remaining(3 * S) > 0.2);
        let before = c.tai_ns(3 * S);
        c.set_live(3 * S, true);
        assert_eq!(c.tai_ns(3 * S), before);
        let r = c.remaining(3 * S);
        assert!((c.remaining(4 * S) - (r - 500e-6)).abs() < 1e-9);
    }

    #[test]
    fn frequency_correction() {
        let mut c = SoftwareClock::new(0, 0, 500e-6, 83_333e-6);
        // Clock 20 ppm fast: slow it down.
        c.accumulate(0, 20e-6, 0.0, 0.0);
        assert_eq!(c.tai_ns(100 * S), 100 * S - 2_000_000);
    }

    #[test]
    fn step_jumps_once() {
        let mut c = SoftwareClock::new(0, 0, 500e-6, 83_333e-6);
        c.step(S, -2.5);
        assert_eq!(c.tai_ns(S), S + 2_500_000_000);
        assert_eq!(c.remaining(S), 0.0);
    }

    #[test]
    fn host_pause_moves_mapping() {
        let mut c = SoftwareClock::new(0, 0, 500e-6, 83_333e-6);
        c.host_paused(10 * S, 3600 * S);
        assert_eq!(c.tai_ns(11 * S), 11 * S + 3600 * S);
    }
}
