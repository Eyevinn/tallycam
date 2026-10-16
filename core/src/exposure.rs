//! Auto ISO: the shutter stays fixed and the ISO follows the metered
//! exposure. Platform auto exposure would change the frame duration, which
//! genlock owns.
//!
//! The platform samples the camera's metering (on iOS
//! `exposureTargetOffset`, EV against the camera's own target) a few times a
//! second and applies the ISO and exposure duration this loop returns.
//!
//! Two properties of the metering shape the loop:
//!
//! - **Its gain is not 1.** On an iPhone a +1 EV target step settled at only
//!   +0.34 EV of ISO: the metered offset moved about 3 EV per EV of exposure.
//!   A loop that assumes 1 overshoots and rings (ISO 448, 632, 545, 566).
//!   This loop learns the gain from its own steps, starts from a high guess
//!   (a step then falls short instead of overshooting), and waits after each
//!   step until the metering has settled before it looks again.
//! - **It reacts to everything in the picture.** A flashing screen in shot
//!   made auto exposure pump. Scene changes only count once they hold for
//!   half the median window (0.6 s); the operator's bias changes act at once.
//!
//! In mains-lit rooms the exposure must be a whole number of light pulses
//! (two per mains cycle) or the picture flickers: [`flicker_free_exposure_ns`]
//! rounds the shutter, and the loop compensates the rounding with the ISO.
//!
//! Pure state machine; no allocation.

use crate::genlock::GenlockConfig;

/// The usual mains frequency for an ISO 3166 region code: 60 Hz in the
/// Americas' 60 Hz countries and a few others, 50 Hz elsewhere. Japan is
/// split (50 Hz east, 60 Hz west) and gets 50.
pub fn mains_hz_for_region(region: &str) -> u32 {
    const SIXTY: &[&str] = &[
        "US", "CA", "MX", "PR", "GU", "VI", "AS", "MP", "UM", "BS", "BZ", "CR", "CU", "DO", "SV",
        "GT", "HT", "HN", "NI", "PA", "BR", "CO", "EC", "PE", "VE", "SR", "GY", "TT", "BM", "KY",
        "AG", "AW", "KN", "LC", "VC", "MS", "TC", "VG", "TW", "KR", "PH", "SA", "LR", "FM", "MH",
        "PW",
    ];
    if SIXTY.iter().any(|r| r.eq_ignore_ascii_case(region)) {
        60
    } else {
        50
    }
}

/// The longest exposure that fits a frame of `frame_period_ns`. Genlock
/// makes frames up to `max_acquire_ppm` shorter than nominal, so leave that
/// much room while it runs.
pub fn longest_exposure_ns(frame_period_ns: f64, format_max_ns: f64, genlock: bool) -> f64 {
    let margin = if genlock {
        GenlockConfig::default().max_acquire_ppm * 1e-6
    } else {
        0.0
    };
    format_max_ns.min(frame_period_ns * (1.0 - margin))
}

/// The flicker-free exposure nearest to `wanted_ns` (in EV): a whole
/// number of half mains periods (10, 20, 30 ms at 50 Hz; 8.3, 16.7, 25 ms
/// at 60 Hz) within `min_ns..=max_ns`. Ties go to the shorter. None when
/// `mains_hz` is 0 or no multiple fits (a frame shorter than half a mains
/// period, 120 fps at 50 Hz).
pub fn flicker_free_exposure_ns(
    wanted_ns: f64,
    mains_hz: u32,
    min_ns: f64,
    max_ns: f64,
) -> Option<f64> {
    if mains_hz == 0 || wanted_ns.is_nan() || wanted_ns <= 0.0 {
        return None;
    }
    let pulse = 1e9 / (2.0 * f64::from(mains_hz));
    // A little slack for durations rounded to whole ns or sensor lines.
    let slack = 1e-6 * pulse;
    let lo = ((min_ns - slack) / pulse).ceil().max(1.0);
    let hi = ((max_ns + slack) / pulse).floor();
    if hi < lo {
        return None;
    }
    let k = (wanted_ns / pulse).clamp(lo, hi);
    let (below, above) = (k.floor().max(lo), k.ceil().min(hi));
    let pick = if (wanted_ns / (below * pulse)).log2() <= ((above * pulse) / wanted_ns).log2() {
        below
    } else {
        above
    };
    Some(pick * pulse)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoIsoConfig {
    /// Median window over the metered offset. A scene change counts once it
    /// fills half the window, so brightness spikes shorter than half of it
    /// (including the metering's own tail) are ignored.
    pub window_ns: i64,
    /// After a step: look again no earlier than this (the ISO reaches the
    /// sensor in a few frames, the metering follows later) ...
    pub min_settle_ns: i64,
    /// ... and once `settle_samples` samples agree within `settle_spread_ev`,
    /// or at the latest after `max_settle_ns`.
    pub max_settle_ns: i64,
    pub settle_samples: usize,
    pub settle_spread_ev: f64,
    /// Scene changes smaller than this are left alone (the picture must not
    /// breathe).
    pub dead_band_ev: f64,
    /// For the operator's bias changes.
    pub bias_dead_band_ev: f64,
    /// Share of the remaining error a step removes, given the learned gain.
    /// Below 1 so a step falls short rather than overshoots.
    pub step_share: f64,
    /// Metered EV per EV of exposure: the starting guess (high, so the
    /// first step falls short) and the range learning keeps it in.
    pub gain_initial: f64,
    pub gain_min: f64,
    pub gain_max: f64,
    /// Largest exposure change per step, EV.
    pub max_step_ev: f64,
    /// 50 or 60: round the shutter to a flicker-free value; 0: leave it.
    pub mains_hz: u32,
}

impl Default for AutoIsoConfig {
    fn default() -> Self {
        Self {
            window_ns: 1_200_000_000,
            min_settle_ns: 300_000_000,
            max_settle_ns: 1_200_000_000,
            settle_samples: 4,
            settle_spread_ev: 0.06,
            dead_band_ev: 0.15,
            bias_dead_band_ev: 0.05,
            step_share: 0.9,
            gain_initial: 3.0,
            gain_min: 0.5,
            gain_max: 4.0,
            max_step_ev: 3.0,
            mains_hz: 50,
        }
    }
}

/// One sample of the camera, a few times a second (8 Hz on iOS).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoIsoInput {
    /// Monotonic time, ns.
    pub t_ns: i64,
    /// Metered exposure against the camera's target, EV (positive: brighter).
    pub offset_ev: f64,
    /// The device's current ISO and exposure duration.
    pub iso: f64,
    pub exposure_ns: f64,
    /// The active format's limits; `max_exposure_ns` from
    /// [`longest_exposure_ns`].
    pub min_iso: f64,
    pub max_iso: f64,
    pub min_exposure_ns: f64,
    pub max_exposure_ns: f64,
}

/// Apply both values in one call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoIsoCommand {
    pub iso: f64,
    pub exposure_ns: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoIsoLimit {
    None,
    /// At the highest ISO and still too dark.
    TooDark,
    /// At the lowest ISO and still too bright.
    TooBright,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoIsoStatus {
    /// Filtered metered offset minus the bias, EV.
    pub error_ev: f64,
    /// Learned metered EV per EV of exposure.
    pub gain: f64,
    pub limit: AutoIsoLimit,
    /// Waiting for the metering after a step.
    pub settling: bool,
}

const CAPACITY: usize = 64;

#[derive(Debug, Clone, Copy)]
enum Phase {
    Idle,
    Settling {
        since: i64,
        /// Exposure change of the step, EV, and the filtered offset before.
        delta_ev: f64,
        offset_before: f64,
        /// Chasing the operator's bias: finish within the bias dead band.
        bias: bool,
    },
}

pub struct AutoIso {
    cfg: AutoIsoConfig,
    bias_ev: f64,
    bias_changed: bool,
    gain: f64,
    phase: Phase,
    /// Samples since this time count for scene changes.
    window_start: i64,
    /// Ring of (time, offset).
    samples: [(i64, f64); CAPACITY],
    len: usize,
    head: usize,
    status: AutoIsoStatus,
}

impl AutoIso {
    pub fn new(cfg: AutoIsoConfig) -> Self {
        Self {
            cfg,
            bias_ev: 0.0,
            bias_changed: false,
            gain: cfg.gain_initial,
            phase: Phase::Idle,
            window_start: i64::MIN,
            samples: [(0, 0.0); CAPACITY],
            len: 0,
            head: 0,
            status: AutoIsoStatus {
                error_ev: 0.0,
                gain: cfg.gain_initial,
                limit: AutoIsoLimit::None,
                settling: false,
            },
        }
    }

    pub fn config(&self) -> &AutoIsoConfig {
        &self.cfg
    }

    pub fn status(&self) -> AutoIsoStatus {
        self.status
    }

    /// Target offset, EV (the operator's exposure compensation).
    pub fn set_bias_ev(&mut self, bias_ev: f64) {
        if bias_ev.is_finite() && bias_ev != self.bias_ev {
            self.bias_ev = bias_ev;
            self.bias_changed = true;
        }
    }

    pub fn bias_ev(&self) -> f64 {
        self.bias_ev
    }

    pub fn set_mains_hz(&mut self, mains_hz: u32) {
        self.cfg.mains_hz = mains_hz;
    }

    /// Starts over (auto ISO turned on, or the device changed). Keeps the
    /// learned gain and the bias.
    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.len = 0;
        self.head = 0;
        self.window_start = i64::MIN;
        self.status.limit = AutoIsoLimit::None;
        self.status.settling = false;
    }

    /// One sample. Returns the ISO and exposure duration to apply, when they
    /// change.
    pub fn on_sample(&mut self, s: &AutoIsoInput) -> Option<AutoIsoCommand> {
        let valid = s.offset_ev.is_finite()
            && s.iso > 0.0
            && s.exposure_ns > 0.0
            && s.min_iso > 0.0
            && s.max_iso >= s.min_iso
            && s.max_exposure_ns >= s.min_exposure_ns;
        if !valid {
            return None;
        }
        if self.window_start == i64::MIN {
            self.window_start = s.t_ns;
        }
        self.push(s.t_ns, s.offset_ev);

        match self.phase {
            Phase::Settling {
                since,
                delta_ev,
                offset_before,
                bias,
            } => {
                let elapsed = s.t_ns - since;
                if elapsed < self.cfg.min_settle_ns {
                    return None;
                }
                let n = self.cfg.settle_samples.max(1);
                let mut recent = [0.0; CAPACITY];
                let mut count = 0;
                let mut first_t = s.t_ns;
                for (t, v) in self.newest(n) {
                    if t >= since + self.cfg.min_settle_ns {
                        recent[count] = v;
                        count += 1;
                        first_t = t;
                    }
                }
                let timed_out = elapsed >= self.cfg.max_settle_ns;
                if count < n && !timed_out {
                    return None;
                }
                if count == 0 {
                    recent[0] = s.offset_ev;
                    count = 1;
                    first_t = s.t_ns;
                }
                let recent = &mut recent[..count];
                let spread = recent.iter().cloned().fold(f64::MIN, f64::max)
                    - recent.iter().cloned().fold(f64::MAX, f64::min);
                let offset = median(recent);
                // The metering must have seen the step: still at the old
                // value is lag, not a settled loop.
                let moved = offset - offset_before;
                let expected = self.gain * delta_ev;
                let followed = moved * expected.signum() >= (0.2 * expected.abs()).min(0.1);
                let settled = spread <= self.cfg.settle_spread_ev && followed;
                if !settled && !timed_out {
                    return None;
                }
                if settled && delta_ev.abs() >= 0.08 && moved.abs() >= 0.05 {
                    // Err high: too low a gain overshoots, too high only
                    // takes another step. Slow metering can still be moving
                    // when it looks settled, so add 10% and lower the gain
                    // at most to a third per step.
                    let measured =
                        (1.1 * moved / delta_ev).clamp(self.cfg.gain_min, self.cfg.gain_max);
                    self.gain = measured.max(self.gain / 3.0);
                    self.status.gain = self.gain;
                }
                self.phase = Phase::Idle;
                self.status.settling = false;
                if !settled {
                    // Timed out (a flash, or a scene still moving): let the
                    // median window decide.
                    self.window_start = since + self.cfg.min_settle_ns;
                    return None;
                }
                self.window_start = first_t;
                let error = offset - self.bias_ev;
                self.status.error_ev = error;
                let band = if bias || self.bias_changed {
                    self.cfg.bias_dead_band_ev
                } else {
                    self.cfg.dead_band_ev
                };
                if error.abs() > band {
                    self.bias_changed |= bias;
                    return self.step(s, error, offset);
                }
                self.status.limit = AutoIsoLimit::None;
                self.neutral(s, offset)
            }
            Phase::Idle => {
                let mut window = [0.0; CAPACITY];
                let mut count = 0;
                let from = self.window_start.max(s.t_ns - self.cfg.window_ns);
                for (t, v) in self.newest(self.len) {
                    if t >= from {
                        window[count] = v;
                        count += 1;
                    }
                }
                let offset = median(&mut window[..count]);
                let error = offset - self.bias_ev;
                self.status.error_ev = error;
                // A scene change only counts with a full window, so it has
                // held for half of it.
                let full = s.t_ns - self.window_start >= self.cfg.window_ns * 9 / 10;
                if self.bias_changed && error.abs() > self.cfg.bias_dead_band_ev
                    || full && error.abs() > self.cfg.dead_band_ev
                {
                    return self.step(s, error, offset);
                }
                if full && error.abs() <= self.cfg.dead_band_ev {
                    self.status.limit = AutoIsoLimit::None;
                }
                self.bias_changed = false;
                self.neutral(s, offset)
            }
        }
    }

    /// The exposure duration to use: flicker-free when mains is set, within
    /// the limits.
    fn duration(&self, s: &AutoIsoInput) -> f64 {
        let current = s.exposure_ns.clamp(s.min_exposure_ns, s.max_exposure_ns);
        flicker_free_exposure_ns(
            current,
            self.cfg.mains_hz,
            s.min_exposure_ns,
            s.max_exposure_ns,
        )
        .filter(|d| (d / s.exposure_ns - 1.0).abs() > 0.005)
        .unwrap_or(current)
    }

    /// Changes the exposure by `-error / gain` (part of it).
    fn step(&mut self, s: &AutoIsoInput, error: f64, offset: f64) -> Option<AutoIsoCommand> {
        let bias = std::mem::take(&mut self.bias_changed);
        let wanted = (-error / self.gain * self.cfg.step_share)
            .clamp(-self.cfg.max_step_ev, self.cfg.max_step_ev);
        let duration = self.duration(s);
        let now = (s.iso * s.exposure_ns).log2();
        let iso_wanted = (now + wanted).exp2() / duration;
        let iso = iso_wanted.clamp(s.min_iso, s.max_iso);
        self.status.limit = if iso_wanted > s.max_iso * 1.001 {
            AutoIsoLimit::TooDark
        } else if iso_wanted < s.min_iso * 0.999 {
            AutoIsoLimit::TooBright
        } else {
            AutoIsoLimit::None
        };
        let delta = (iso * duration).log2() - now;
        if delta.abs() < 0.03 && duration == s.exposure_ns {
            return None;
        }
        self.begin(s.t_ns, delta, offset, bias);
        Some(AutoIsoCommand {
            iso,
            exposure_ns: duration,
        })
    }

    /// Rounds the shutter without changing the exposure.
    fn neutral(&mut self, s: &AutoIsoInput, offset: f64) -> Option<AutoIsoCommand> {
        let duration = self.duration(s);
        if duration == s.exposure_ns {
            return None;
        }
        let iso = (s.iso * s.exposure_ns / duration).clamp(s.min_iso, s.max_iso);
        let delta = (iso * duration / (s.iso * s.exposure_ns)).log2();
        self.begin(s.t_ns, delta, offset, false);
        Some(AutoIsoCommand {
            iso,
            exposure_ns: duration,
        })
    }

    fn begin(&mut self, t: i64, delta_ev: f64, offset_before: f64, bias: bool) {
        self.phase = Phase::Settling {
            since: t,
            delta_ev,
            offset_before,
            bias,
        };
        self.status.settling = true;
    }

    fn push(&mut self, t: i64, v: f64) {
        self.samples[self.head] = (t, v);
        self.head = (self.head + 1) % CAPACITY;
        self.len = (self.len + 1).min(CAPACITY);
    }

    /// Up to `n` samples, newest first.
    fn newest(&self, n: usize) -> impl Iterator<Item = (i64, f64)> + '_ {
        (1..=n.min(self.len)).map(move |i| self.samples[(self.head + CAPACITY - i) % CAPACITY])
    }
}

/// Median of a small slice, newest first (sorted in place); 0 when empty.
/// An even count drops the oldest sample: averaging the middle two would
/// let a spike that fills half the window pull the result halfway.
fn median(v: &mut [f64]) -> f64 {
    let n = if v.len().is_multiple_of(2) {
        v.len().saturating_sub(1)
    } else {
        v.len()
    };
    if n == 0 {
        return 0.0;
    }
    let v = &mut v[..n];
    v.sort_unstable_by(f64::total_cmp);
    v[n / 2]
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;

    /// A camera with metering: the ISO and duration reach the sensor
    /// `apply_ms` after they are set; the metered offset is
    /// `gain * (exposure EV - target)` plus the scene, seen `delay_ms` later
    /// through a first-order lag of `tau_ms`, plus noise.
    struct Camera {
        t: i64,
        gain: f64,
        apply_ms: i64,
        delay_ms: i64,
        tau_ms: f64,
        noise_ev: f64,
        /// Scene brightness, EV, by time.
        scene: Box<dyn Fn(i64) -> f64>,
        /// (applies at, iso, duration ns)
        pending: Vec<(i64, f64, f64)>,
        iso: f64,
        exposure_ns: f64,
        /// Exposure EV history per ms for the metering delay.
        history: std::collections::VecDeque<f64>,
        metered: f64,
        seed: u64,
    }

    /// Scene 0 and ISO 448 at 1/100 s is on target.
    const REF: f64 = 448.0 * 10_000_000.0;

    impl Camera {
        fn new(gain: f64, delay_ms: i64, tau_ms: f64) -> Self {
            Self {
                t: 0,
                gain,
                apply_ms: 120,
                delay_ms,
                tau_ms,
                noise_ev: 0.02,
                scene: Box::new(|_| 0.0),
                pending: Vec::new(),
                iso: 448.0,
                exposure_ns: 10_000_000.0,
                history: std::collections::VecDeque::new(),
                metered: 0.0,
                seed: 0x9e37_79b9_7f4a_7c15,
            }
        }

        fn noise(&mut self) -> f64 {
            self.seed ^= self.seed >> 12;
            self.seed ^= self.seed << 25;
            self.seed ^= self.seed >> 27;
            let r = self.seed.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11;
            r as f64 / (1u64 << 52) as f64 - 1.0
        }

        /// Exposure EV of the picture now (0: on the original target).
        fn picture_ev(&self) -> f64 {
            (self.iso * self.exposure_ns / REF).log2() + (self.scene)(self.t)
        }

        fn set(&mut self, iso: f64, exposure_ns: f64) {
            self.pending
                .push((self.t + self.apply_ms * MS, iso, exposure_ns));
        }

        /// Advances 1 ms.
        fn tick(&mut self) {
            self.t += MS;
            let t = self.t;
            if let Some(i) = self.pending.iter().rposition(|p| p.0 <= t) {
                self.iso = self.pending[i].1;
                self.exposure_ns = self.pending[i].2;
                self.pending.drain(..=i);
            }
            let ev = self.picture_ev();
            self.history.push_back(ev);
            if self.history.len() > self.delay_ms as usize + 1 {
                self.history.pop_front();
            }
            let seen = self.history[0];
            let a = if self.tau_ms > 0.0 {
                1.0 / self.tau_ms
            } else {
                1.0
            };
            self.metered += (seen - self.metered) * a.min(1.0);
        }

        fn input(&mut self) -> AutoIsoInput {
            let n = self.noise() * self.noise_ev;
            AutoIsoInput {
                t_ns: self.t,
                offset_ev: self.gain * self.metered + n,
                iso: self.iso,
                exposure_ns: self.exposure_ns,
                min_iso: 34.0,
                max_iso: 3264.0,
                min_exposure_ns: 14_000.0,
                max_exposure_ns: 33_333_333.0,
            }
        }
    }

    /// Runs the loop at 8 Hz for `ms`; `each` sees every ms.
    fn run(cam: &mut Camera, ai: &mut AutoIso, ms: i64, mut each: impl FnMut(&Camera, &AutoIso)) {
        for _ in 0..ms {
            cam.tick();
            if cam.t % (125 * MS) == 0 {
                let input = cam.input();
                if let Some(c) = ai.on_sample(&input) {
                    if std::env::var("AI_TRACE").is_ok() {
                        eprintln!(
                            "{} ms off {:.2} iso {:.0} -> {:.0} dur {:.2} ms gain {:.2} {:?}",
                            cam.t / MS,
                            input.offset_ev,
                            cam.iso,
                            c.iso,
                            c.exposure_ns / 1e6,
                            ai.gain,
                            ai.status()
                        );
                    }
                    cam.set(c.iso, c.exposure_ns);
                }
            }
            each(cam, ai);
        }
    }

    /// The loop that ran in Swift until this module: 4 Hz, half the error
    /// per step, waits for the device to apply each step.
    fn old_loop(cam: &mut Camera, bias: f64, ms: i64) -> Vec<f64> {
        let mut isos = vec![cam.iso];
        let mut in_flight_until = 0;
        for _ in 0..ms {
            cam.tick();
            if cam.t % (250 * MS) != 0 || cam.t < in_flight_until {
                continue;
            }
            let error = cam.input().offset_ev - bias;
            if error.abs() <= 0.15 {
                continue;
            }
            let iso = cam.iso * (-0.5 * error.clamp(-2.0, 2.0)).exp2();
            cam.set(iso, cam.exposure_ns);
            in_flight_until = cam.t + cam.apply_ms * MS;
            isos.push(iso);
        }
        isos
    }

    fn settled(ms: i64) -> (Camera, AutoIso) {
        let mut cam = Camera::new(3.0, 30, 50.0);
        let mut ai = AutoIso::new(AutoIsoConfig::default());
        run(&mut cam, &mut ai, ms, |_, _| {});
        (cam, ai)
    }

    #[test]
    fn the_camera_model_reproduces_the_old_loops_ringing() {
        // Measured on an iPhone after a +1 EV bias step: ISO 448, 632, 545,
        // 566. A metering gain of 3 gives the same sequence.
        let mut cam = Camera::new(3.0, 10, 20.0);
        cam.noise_ev = 0.0;
        let isos = old_loop(&mut cam, 1.0, 3000);
        let want = [448.0, 632.0, 545.0, 566.0];
        assert!(isos.len() >= 4, "{isos:?}");
        for (got, want) in isos.iter().zip(want) {
            assert!((got / want - 1.0).abs() < 0.04, "{isos:?}");
        }
        // The picture went 1.5 EV up before settling near 1.
    }

    /// Steps the bias by `bias` after a settled second and returns the
    /// picture's exposure over time (EV, ms after the step).
    fn bias_step(
        gain: f64,
        delay_ms: i64,
        tau_ms: f64,
        learned: bool,
        bias: f64,
    ) -> (Vec<f64>, f64) {
        let mut cam = Camera::new(gain, delay_ms, tau_ms);
        let mut ai = AutoIso::new(AutoIsoConfig {
            mains_hz: 0,
            ..Default::default()
        });
        run(&mut cam, &mut ai, 2000, |_, _| {});
        if learned {
            // One step there and back teaches the gain.
            ai.set_bias_ev(0.5 * gain);
            run(&mut cam, &mut ai, 3000, |_, _| {});
            ai.set_bias_ev(0.0);
            run(&mut cam, &mut ai, 3000, |_, _| {});
        }
        let start = cam.picture_ev();
        // The bias is in metered EV; the picture should move bias / gain.
        ai.set_bias_ev(bias);
        let mut trace = Vec::new();
        run(&mut cam, &mut ai, 4000, |c, _| {
            trace.push(c.picture_ev() - start)
        });
        (trace, bias / gain)
    }

    fn settle_ms(trace: &[f64], target: f64, tolerance: f64) -> usize {
        trace
            .iter()
            .rposition(|e| (e - target).abs() > tolerance)
            .map_or(0, |i| i + 1)
    }

    #[test]
    fn a_bias_step_settles_without_overshoot() {
        for &(gain, delay, tau) in &[
            (3.0, 30, 50.0),
            (1.0, 30, 50.0),
            (2.0, 100, 200.0),
            (1.0, 200, 300.0),
            (0.7, 100, 150.0),
            (4.0, 50, 100.0),
        ] {
            for learned in [false, true] {
                for bias in [1.0, -1.0] {
                    let (trace, target) = bias_step(gain, delay, tau, learned, bias);
                    let tolerance = 0.15 / gain + 0.05;
                    let t = settle_ms(&trace, target, tolerance);
                    let overshoot = trace
                        .iter()
                        .map(|e| (e - target) * target.signum())
                        .fold(f64::MIN, f64::max);
                    let case = format!(
                        "gain {gain} delay {delay} tau {tau} learned {learned} bias {bias}"
                    );
                    assert!(overshoot < 0.1, "{case}: overshoot {overshoot:.2} EV");
                    // Unlearned, the first step falls short and a second one follows;
                    // slow metering stretches both.
                    let lag = (delay + tau as i64) as usize;
                    let limit = if learned {
                        1000 + 2 * lag
                    } else {
                        1500 + 3 * lag
                    };
                    if std::env::var("AI_STEPS").is_ok() {
                        eprintln!("{case}: settled {t} ms, overshoot {overshoot:.3} EV");
                    }
                    assert!(t <= limit, "{case}: settled after {t} ms");
                }
            }
        }
    }

    #[test]
    fn step_response_with_the_measured_gain() {
        // The case measured on the phone: gain 3, short metering lag.
        let (trace, target) = bias_step(3.0, 30, 50.0, false, 1.0);
        let t = settle_ms(&trace, target, 0.1);
        assert!(t <= 600, "settled after {t} ms");
        let peak = trace.iter().cloned().fold(f64::MIN, f64::max);
        assert!(peak < target + 0.05, "peak {peak:.2} target {target:.2}");
    }

    #[test]
    fn ignores_brief_flashes() {
        let (mut cam, mut ai) = settled(3000);
        let iso = cam.iso;
        // 0.4 s at +1 EV (3 EV metered), then a screen flashing 0.3 s in every
        // second.
        cam.scene = Box::new(|t| {
            let ms = t / MS;
            if (3000..3400).contains(&ms) || (5000..20_000).contains(&ms) && ms % 1000 < 300 {
                1.0
            } else {
                0.0
            }
        });
        let mut commands = 0;
        let mut last = iso;
        run(&mut cam, &mut ai, 17_000, |c, _| {
            if c.iso != last {
                commands += 1;
                last = c.iso;
            }
        });
        assert_eq!(commands, 0, "ISO moved to {last}");
    }

    #[test]
    fn follows_a_lasting_scene_change() {
        let (mut cam, mut ai) = settled(3000);
        cam.scene = Box::new(|t| if t >= 3000 * MS { -1.0 } else { 0.0 });
        let mut trace = Vec::new();
        run(&mut cam, &mut ai, 4000, |c, _| trace.push(c.picture_ev()));
        let t = settle_ms(&trace, 0.0, 0.1);
        assert!(t <= 1700, "settled after {t} ms");
        assert!(trace.iter().all(|e| *e < 0.1), "overshoot");
    }

    #[test]
    fn warns_at_the_iso_limit() {
        let (mut cam, mut ai) = settled(2000);
        cam.scene = Box::new(|t| if t >= 2000 * MS { -6.0 } else { 0.0 });
        run(&mut cam, &mut ai, 6000, |_, _| {});
        assert_eq!(cam.iso, 3264.0);
        assert_eq!(ai.status().limit, AutoIsoLimit::TooDark);
        cam.scene = Box::new(|_| 0.0);
        run(&mut cam, &mut ai, 6000, |_, _| {});
        assert_eq!(ai.status().limit, AutoIsoLimit::None);
        assert!((cam.picture_ev()).abs() < 0.1);
    }

    #[test]
    fn rounds_the_shutter_and_keeps_the_exposure() {
        let mut cam = Camera::new(3.0, 30, 50.0);
        cam.exposure_ns = 4_000_000.0; // 1/250
        cam.iso = 448.0 * 2.5;
        let mut ai = AutoIso::new(AutoIsoConfig::default());
        let before = cam.picture_ev();
        let mut worst = 0f64;
        run(&mut cam, &mut ai, 3000, |c, _| {
            worst = worst.max((c.picture_ev() - before).abs())
        });
        assert_eq!(cam.exposure_ns, 10_000_000.0);
        assert!((cam.iso - 448.0).abs() < 1.0, "{}", cam.iso);
        assert!(worst < 0.01);
        // At 60 Hz.
        ai.set_mains_hz(60);
        run(&mut cam, &mut ai, 3000, |_, _| {});
        assert!((cam.exposure_ns - 1e9 / 120.0).abs() < 1.0);
    }

    #[test]
    fn flicker_free_values() {
        let ms = 1_000_000.0;
        let ff = |w: f64, hz, max: f64| {
            flicker_free_exposure_ns(w * ms, hz, 0.014 * ms, max * ms).map(|d| d / ms)
        };
        // 50 Hz at 30 fps: 10, 20, 30 ms.
        assert_eq!(ff(4.0, 50, 33.3), Some(10.0));
        assert_eq!(ff(1000.0 / 60.0, 50, 33.3), Some(20.0));
        assert_eq!(ff(33.3, 50, 33.3), Some(30.0));
        assert_eq!(
            ff(14.0, 50, 33.3),
            Some(10.0),
            "14 ms is nearer 10 than 20 in EV"
        );
        assert_eq!(ff(15.0, 50, 33.3), Some(20.0));
        // 60 Hz: 1/120, 1/60, 1/40, and 1/30 fits a 30 fps frame.
        let d = ff(10.0, 60, 33.34).unwrap();
        assert!((d - 1000.0 / 120.0).abs() < 1e-9, "{d}");
        assert!((ff(30.0, 60, 33.34).unwrap() - 1000.0 / 30.0).abs() < 1e-9);
        assert!((ff(25.0, 60, 33.34).unwrap() - 25.0).abs() < 1e-9);
        // 25 fps at 50 Hz: 40 ms fits, but not while genlock may shorten
        // frames.
        assert_eq!(ff(40.0, 50, 40.0), Some(40.0));
        let longest = longest_exposure_ns(40.0 * ms, 1e9, true) / ms;
        assert_eq!(ff(40.0, 50, longest), Some(30.0));
        assert_eq!(longest_exposure_ns(40.0 * ms, 1e9, false), 40.0 * ms);
        assert_eq!(longest_exposure_ns(40.0 * ms, 20.0 * ms, false), 20.0 * ms);
        // 120 fps at 50 Hz: no flicker-free shutter fits.
        assert_eq!(ff(4.0, 50, 8.33), None);
        assert_eq!(ff(4.0, 0, 33.3), None);
        // A minimum above the first pulse.
        assert_eq!(
            flicker_free_exposure_ns(5.0 * ms, 50, 12.0 * ms, 33.0 * ms),
            Some(20.0 * ms)
        );
    }

    #[test]
    fn mains_by_region() {
        assert_eq!(mains_hz_for_region("US"), 60);
        assert_eq!(mains_hz_for_region("br"), 60);
        assert_eq!(mains_hz_for_region("SE"), 50);
        assert_eq!(mains_hz_for_region("JP"), 50);
        assert_eq!(mains_hz_for_region(""), 50);
    }
}
