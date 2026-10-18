//! Soft genlock: locks a camera's frame timing to the TAI frame grid.
//!
//! Every camera aims at the same grid: a frame on each whole TAI second and
//! then every `1 / fps`. Each frame's capture time (TAI, from the clock
//! discipline) gives its phase error against that grid. Once per window
//! (about a second of frames) a PI loop turns the mean error into a frame
//! duration to ask the sensor for: slightly shorter to catch up, slightly
//! longer to fall back. The integral term learns the sensor's own rate error
//! (a phone measured -75 ppm at 30 fps) and follows it as it drifts with
//! temperature. Cameras never talk to each other: locking to the same grid
//! puts them in phase with each other.
//!
//! Pure state machine: the platform reports frames and applies the duration
//! it returns. No allocation; `on_frame` is cheap enough for every frame.

/// Tuning and limits. The defaults suit 24 to 60 fps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenlockConfig {
    /// Nominal frame rate as a fraction (30/1, 30000/1001).
    pub fps_num: u32,
    pub fps_den: u32,
    /// Where on the grid the frames should land, ns after each grid point.
    pub target_offset_ns: i64,
    /// Frames per control window; 0 means one second's worth.
    pub window_frames: u32,
    /// Share of the mean phase error removed per window (proportional).
    pub kp: f64,
    /// Integral gain per window.
    pub ki: f64,
    /// Largest correction while acquiring, ppm of the period.
    pub max_acquire_ppm: f64,
    /// Largest correction once locked, ppm of the period.
    pub max_track_ppm: f64,
    /// Never ask for a frame shorter than this (the format's minimum).
    pub min_frame_duration_ns: f64,
    /// Locked after `lock_windows` windows with a mean error below this.
    pub lock_threshold_ns: f64,
    pub lock_windows: u32,
    /// Back to acquiring when a window's mean error exceeds this.
    pub unlock_threshold_ns: f64,
}

impl Default for GenlockConfig {
    fn default() -> Self {
        Self {
            fps_num: 30,
            fps_den: 1,
            target_offset_ns: 0,
            window_frames: 0,
            kp: 0.3,
            ki: 0.05,
            max_acquire_ppm: 20_000.0,
            max_track_ppm: 2_000.0,
            min_frame_duration_ns: 0.0,
            lock_threshold_ns: 500_000.0,
            lock_windows: 3,
            unlock_threshold_ns: 2_000_000.0,
        }
    }
}

impl GenlockConfig {
    /// Nominal frame period, ns.
    pub fn period_ns(&self) -> f64 {
        1e9 * f64::from(self.fps_den) / f64::from(self.fps_num)
    }

    fn window(&self) -> u32 {
        if self.window_frames > 0 {
            self.window_frames
        } else {
            (self.fps_num / self.fps_den.max(1)).max(1)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenlockState {
    /// No trusted time: hold the learned rate, do not steer the phase.
    FreeRun,
    /// Pulling the phase onto the grid.
    Acquiring,
    /// On the grid within `lock_threshold_ns`.
    Locked,
}

impl GenlockState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FreeRun => "free_run",
            Self::Acquiring => "acquiring",
            Self::Locked => "locked",
        }
    }
}

/// What the loop reports, refreshed once per window.
///
/// A sensor sets frame durations in whole lines, so it cannot ask for the
/// few microseconds a rate error needs; the loop then alternates between
/// neighbouring steps and the phase wanders by about one step per window
/// frame (0.2 ms with 7.4 us lines at 30 fps).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GenlockStatus {
    pub state: GenlockState,
    /// Mean phase error of the last window, ns (positive: frames late).
    pub phase_error_ns: f64,
    /// Largest |phase error| of a single frame in the last window, ns.
    pub phase_error_max_ns: f64,
    /// The frame duration asked for, ns.
    pub frame_duration_ns: f64,
    /// Correction against the nominal period, ppm (negative: shorter).
    pub correction_ppm: f64,
    /// Catching a late phase up on the next grid point by lengthening
    /// frames, because the format cannot run fast enough to trim it.
    pub slipping: bool,
    /// Frames at the format's shortest duration still land later each
    /// window: this sensor is too slow to hold the grid on this format. The
    /// phase drifts late and wraps a frame about every half period / rate
    /// error; a format that runs faster than nominal fixes it.
    pub format_too_slow: bool,
}

pub struct Genlock {
    cfg: GenlockConfig,
    period: f64,
    state: GenlockState,
    /// Correction asked for, ns per frame, and its integral part.
    correction: f64,
    integral: f64,
    /// The current window.
    count: u32,
    sum: f64,
    max_abs: f64,
    last_error: Option<f64>,
    /// The previous window's mean error, to tell a held offset from a pull.
    last_mean: Option<f64>,
    good_windows: u32,
    /// Durations published in a row at the format's minimum, windows in a
    /// row the phase moved later (positive) or earlier there, and the
    /// verdict drawn from them.
    floor_windows: u32,
    floor_trend: i32,
    too_slow: bool,
    status: GenlockStatus,
}

impl Genlock {
    pub fn new(cfg: GenlockConfig) -> Self {
        let period = cfg.period_ns();
        Self {
            cfg,
            period,
            state: GenlockState::FreeRun,
            correction: 0.0,
            integral: 0.0,
            count: 0,
            sum: 0.0,
            max_abs: 0.0,
            last_error: None,
            last_mean: None,
            good_windows: 0,
            floor_windows: 0,
            floor_trend: 0,
            too_slow: false,
            status: GenlockStatus {
                state: GenlockState::FreeRun,
                phase_error_ns: 0.0,
                phase_error_max_ns: 0.0,
                frame_duration_ns: period,
                correction_ppm: 0.0,
                slipping: false,
                format_too_slow: false,
            },
        }
    }

    pub fn config(&self) -> &GenlockConfig {
        &self.cfg
    }

    /// Moves the target on the grid (per-model calibration, or sweeps that
    /// measure it). The learned rate is kept; the loop pulls the phase over.
    pub fn set_target_offset_ns(&mut self, offset_ns: i64) {
        if offset_ns != self.cfg.target_offset_ns {
            self.cfg.target_offset_ns = offset_ns;
            self.last_error = None;
            self.reset_window();
        }
    }

    pub fn status(&self) -> GenlockStatus {
        self.status
    }

    /// The frame duration to ask for now, ns.
    pub fn frame_duration_ns(&self) -> f64 {
        self.status.frame_duration_ns
    }

    /// Phase error of a capture time against the grid, ns, in
    /// [-period/2, period/2). Exact for any TAI time: the modulo is taken in
    /// integers before dividing.
    pub fn phase_error_ns(&self, capture_tai_ns: i64) -> f64 {
        let num = i128::from(self.cfg.fps_num);
        let modulus = 1_000_000_000i128 * i128::from(self.cfg.fps_den);
        let t = i128::from(capture_tai_ns) - i128::from(self.cfg.target_offset_ns);
        let scaled = (t * num).rem_euclid(modulus);
        let mut e = scaled as f64 / num as f64;
        if e >= self.period / 2.0 {
            e -= self.period;
        }
        e
    }

    /// One captured frame. `synced` is the clock's verdict (the SEI's synced
    /// flag). Returns a new frame duration (ns) at the end of each window;
    /// the platform applies it when it differs from the current one.
    pub fn on_frame(&mut self, capture_tai_ns: i64, synced: bool) -> Option<f64> {
        if !synced {
            if self.state != GenlockState::FreeRun {
                self.state = GenlockState::FreeRun;
                self.good_windows = 0;
                self.reset_window();
                self.last_mean = None;
                // Keep the learned rate, drop the phase steering.
                self.correction = self.integral;
                return Some(self.publish(self.status.phase_error_ns, 0.0, false));
            }
            return None;
        }
        if self.state == GenlockState::FreeRun {
            self.state = GenlockState::Acquiring;
        }

        // Unwrap against the previous frame so a window that crosses the
        // edge of the grid does not average +P/2 and -P/2 to zero.
        let mut e = self.phase_error_ns(capture_tai_ns);
        if let Some(prev) = self.last_error {
            let d = e - prev;
            if d > self.period / 2.0 {
                e -= self.period;
            } else if d < -self.period / 2.0 {
                e += self.period;
            }
            if (e - prev).abs() > self.period / 4.0 {
                // A clock step or a capture restart, not drift.
                self.reset_window();
                self.last_error = Some(self.phase_error_ns(capture_tai_ns));
                return None;
            }
        }
        self.last_error = Some(e);
        self.sum += e;
        self.max_abs = self.max_abs.max(e.abs());
        self.count += 1;
        if self.count < self.cfg.window() {
            return None;
        }

        let n = f64::from(self.count);
        // Fast acquisition can carry the unwrapped error past the grid edge
        // within one window: fold the mean back onto [-P/2, P/2).
        let mean = self.fold(self.sum / n);
        let max_abs = self.max_abs;
        // Keep the unwrapped error continuous into the next window, but
        // bring it back onto [-P/2, P/2) once the window is closed.
        self.last_error = Some(self.phase_error_ns(capture_tai_ns));
        self.reset_window();

        self.judge_floor(mean);
        self.update_lock(mean);
        // A format that cannot run faster than nominal can only lengthen
        // frames: catch a late phase up on the next grid point instead. The
        // room to shorten counts from the learned rate, not from nominal: a
        // fast sensor already runs on lengthened frames and can give some
        // back (an iPad 6 at 1080p30 sits at about +1600 ppm).
        let headroom = self.period + self.integral - self.cfg.min_frame_duration_ns;
        // Slip only when trimming cannot catch up within ten windows; a slip
        // is a whole frame of disturbance, trimming is invisible. Never for
        // a sensor too slow for the format: it would drift late again at
        // once (a slip every 13 s at +75 ppm). Left to drift, it wraps on
        // its own at half a period and is pulled back from the early side.
        let slip = !self.too_slow
            && mean > self.cfg.lock_threshold_ns
            && headroom.max(0.0) * n < 0.1 * mean;
        let control = if slip { mean - self.period } else { mean };
        let limit = self.period
            * 1e-6
            * if self.state == GenlockState::Locked {
                self.cfg.max_track_ppm
            } else {
                self.cfg.max_acquire_ppm
            };
        // Positive error: frames land late, so make frames shorter. Both
        // terms are per-frame corrections; spreading the error over the
        // window's frames makes the gains independent of the frame rate.
        let proportional = -self.cfg.kp * control / n;
        // The integral learns the sensor's rate near the grid, or wherever
        // the phase holds still: while it is being pulled across, the error
        // says nothing about the rate, but a steady offset is exactly the
        // rate the proportional term is covering (a sensor 1600 ppm fast
        // holds about 5 ms off with the proportional term alone). Never
        // while slipping to the next grid point.
        let holding = self
            .last_mean
            .is_some_and(|prev| (mean - prev).abs() < self.cfg.lock_threshold_ns);
        self.last_mean = Some(mean);
        let learning = !slip && (control.abs() < self.cfg.unlock_threshold_ns || holding);
        let integral = if learning {
            self.integral - self.cfg.ki * control / n
        } else {
            self.integral
        };
        let wanted = proportional + integral;
        let clamped = wanted.clamp(-limit, limit);
        // Anti-windup: only integrate while the output is not saturated,
        // and never below the shortest frame the format allows (frames at
        // the minimum cannot get any shorter, so the rate learned there
        // would only run away).
        let floor = (self.cfg.min_frame_duration_ns - self.period).max(-limit);
        if (wanted - clamped).abs() < f64::EPSILON {
            let mut next = integral.clamp(floor.min(limit), limit);
            // The minimum saturates the output too. While frames there still
            // catch the phase up, the learned rate is the headroom the trim
            // runs on: unlearning it would run it down to the floor and turn
            // a 0.7 ms trim into a slip. Only a sensor too slow for the
            // format unlearns it there.
            if clamped < floor && !self.too_slow {
                next = next.max(self.integral.min(limit));
            }
            self.integral = next;
        }
        self.correction = clamped;
        Some(self.publish(mean, max_abs, slip))
    }

    /// Watches the phase while frames run at the format's minimum. A
    /// verdict takes three windows in a row the same way, so a clock step
    /// or the noise of one window does not flip it.
    fn judge_floor(&mut self, mean: f64) {
        // Two durations at the minimum in a row: the last two windows both
        // ran there, so their difference is the floor's own drift.
        match self.last_mean {
            Some(prev) if self.floor_windows >= 2 => {
                self.floor_trend = if self.fold(mean - prev) > 0.0 {
                    self.floor_trend.max(0) + 1
                } else {
                    self.floor_trend.min(0) - 1
                };
                if self.floor_trend >= 3 {
                    self.too_slow = true;
                } else if self.floor_trend <= -3 {
                    self.too_slow = false;
                }
            }
            _ => self.floor_trend = 0,
        }
    }

    fn update_lock(&mut self, mean: f64) {
        match self.state {
            GenlockState::Locked => {
                // No hysteresis for a phase that cannot be held: it would
                // stay "locked" while drifting to the unlock threshold.
                let drifting = self.too_slow && mean > self.cfg.lock_threshold_ns;
                if mean.abs() > self.cfg.unlock_threshold_ns || drifting {
                    self.state = GenlockState::Acquiring;
                    self.good_windows = 0;
                }
            }
            _ => {
                if mean.abs() < self.cfg.lock_threshold_ns {
                    self.good_windows += 1;
                    if self.good_windows >= self.cfg.lock_windows {
                        self.state = GenlockState::Locked;
                    }
                } else {
                    self.good_windows = 0;
                }
            }
        }
    }

    /// Folds an error onto [-period/2, period/2).
    fn fold(&self, e: f64) -> f64 {
        e - self.period * ((e + self.period / 2.0) / self.period).floor()
    }

    fn publish(&mut self, mean: f64, max_abs: f64, slipping: bool) -> f64 {
        let duration = (self.period + self.correction).max(self.cfg.min_frame_duration_ns);
        if duration <= self.cfg.min_frame_duration_ns {
            self.floor_windows += 1;
        } else {
            self.floor_windows = 0;
        }
        self.status = GenlockStatus {
            state: self.state,
            phase_error_ns: mean,
            phase_error_max_ns: max_abs,
            frame_duration_ns: duration,
            correction_ppm: (duration - self.period) / self.period * 1e6,
            slipping,
            format_too_slow: self.too_slow,
        };
        duration
    }

    fn reset_window(&mut self) {
        self.count = 0;
        self.sum = 0.0;
        self.max_abs = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sensor that runs `bias_ppm` off whatever duration it is asked for,
    /// rounds durations to `step_ns` (whole sensor lines), applies a new
    /// duration `delay` frames late and stamps with `jitter_ns` of noise.
    struct Sensor {
        t: f64,
        asked: f64,
        pending: [(u32, f64); 4],
        frame: u32,
        bias_ppm: f64,
        step_ns: f64,
        delay: u32,
        jitter_ns: f64,
        seed: u64,
    }

    impl Sensor {
        fn new(start_ns: f64, period: f64) -> Self {
            Self {
                t: start_ns,
                asked: period,
                pending: [(u32::MAX, 0.0); 4],
                frame: 0,
                bias_ppm: -75.0,
                step_ns: 7_400.0,
                delay: 3,
                jitter_ns: 60_000.0,
                seed: 0x9e37_79b9_7f4a_7c15,
            }
        }

        fn noise(&mut self) -> f64 {
            // xorshift64*, uniform in [-1, 1).
            self.seed ^= self.seed >> 12;
            self.seed ^= self.seed << 25;
            self.seed ^= self.seed >> 27;
            let r = self.seed.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11;
            r as f64 / (1u64 << 52) as f64 - 1.0
        }

        fn ask(&mut self, duration: f64) {
            let slot = self.pending.iter_mut().find(|p| p.0 == u32::MAX).unwrap();
            *slot = (self.frame + self.delay, duration);
        }

        /// Next frame's stamped capture time.
        fn next(&mut self) -> i64 {
            for p in &mut self.pending {
                if p.0 != u32::MAX && p.0 <= self.frame {
                    self.asked = p.1;
                    p.0 = u32::MAX;
                }
            }
            let actual = (self.asked / self.step_ns).round() * self.step_ns;
            self.t += actual * (1.0 + self.bias_ppm * 1e-6);
            self.frame += 1;
            (self.t + self.jitter_ns * self.noise()) as i64
        }
    }

    const T0: f64 = 1.8e18; // a TAI time in 2027, ns

    fn run(cfg: GenlockConfig, seconds: u32, mut each: impl FnMut(u32, &mut Sensor, &Genlock)) {
        let mut g = Genlock::new(cfg);
        let mut s = Sensor::new(T0 + 13_000_000.0, cfg.period_ns());
        for i in 0..seconds * cfg.fps_num / cfg.fps_den {
            let t = s.next();
            if let Some(d) = g.on_frame(t, true) {
                s.ask(d);
            }
            each(i, &mut s, &g);
        }
    }

    /// The line time of a format whose shortest frame is `period`, about
    /// 7.4 us: a format's shortest frame is a whole number of lines.
    fn lines_of(period: f64) -> f64 {
        period / (period / 7_400.0).round()
    }

    /// What a run did, seen from the sensor and from the loop's windows.
    #[derive(Debug, Default)]
    struct Tally {
        /// When (s into the run) the sensor moved on to another grid point:
        /// a whole frame dropped or repeated against the grid.
        frame_slips: Vec<f64>,
        windows: u32,
        slip_windows: u32,
        locked_windows: u32,
        /// Largest |mean error| of a locked window, ns.
        worst_locked_ns: f64,
    }

    /// Feeds `seconds` of frames from `s` to `g`, calling `each` before
    /// every frame (to step or bend the sensor).
    fn tally(
        g: &mut Genlock,
        s: &mut Sensor,
        seconds: u32,
        mut each: impl FnMut(u32, &mut Sensor, &Genlock),
    ) -> Tally {
        let cfg = *g.config();
        let period = cfg.period_ns();
        let grid_point = |s: &Sensor| ((s.t - T0) / period).round() as i64 - i64::from(s.frame);
        let mut out = Tally::default();
        let mut point = grid_point(s);
        for i in 0..seconds * cfg.fps_num / cfg.fps_den {
            each(i, s, g);
            let t = s.next();
            if let Some(d) = g.on_frame(t, true) {
                s.ask(d);
                let st = g.status();
                out.windows += 1;
                out.slip_windows += u32::from(st.slipping);
                if std::env::var("GL_TRACE").is_ok() {
                    eprintln!(
                        "{} {:?} e {:.0} us c {:.1} ppm slip {} slow {} fw {} tr {}",
                        i / 30,
                        st.state,
                        st.phase_error_ns / 1e3,
                        st.correction_ppm,
                        st.slipping,
                        st.format_too_slow,
                        g.floor_windows,
                        g.floor_trend
                    );
                }
                if st.state == GenlockState::Locked {
                    out.locked_windows += 1;
                    out.worst_locked_ns = out.worst_locked_ns.max(st.phase_error_ns.abs());
                }
            }
            if grid_point(s) != point {
                point = grid_point(s);
                out.frame_slips.push(f64::from(i) * period / 1e9);
            }
        }
        out
    }

    #[test]
    fn phase_error_is_exact_and_wraps() {
        let g = Genlock::new(GenlockConfig::default());
        let p = 1e9 / 30.0;
        let second = 1_800_000_000_000_000_000i64;
        assert_eq!(g.phase_error_ns(second), 0.0);
        assert!((g.phase_error_ns(second + 1_000) - 1_000.0).abs() < 1e-6);
        assert!((g.phase_error_ns(second - 1_000) + 1_000.0).abs() < 1e-6);
        // Just past half a period counts as early for the next frame.
        let e = g.phase_error_ns(second + (p / 2.0) as i64 + 10);
        assert!(e < 0.0 && e > -p / 2.0, "{e}");
        // Frame 7 of the second, 2 ms late.
        let e = g.phase_error_ns(second + (7.0 * p) as i64 + 2_000_000);
        assert!((e - 2_000_000.0).abs() < 1.0, "{e}");

        let ntsc = Genlock::new(GenlockConfig {
            fps_num: 30_000,
            fps_den: 1_001,
            ..Default::default()
        });
        // The 29.97 grid is anchored at the TAI epoch, not at each second:
        // 900 frames on (30.03 s) is the same phase.
        let base = ntsc.phase_error_ns(second);
        assert!((ntsc.phase_error_ns(second + 30_030_000_000) - base).abs() < 1e-3);
    }

    #[test]
    fn locks_a_sensor_running_75_ppm_fast() {
        let cfg = GenlockConfig::default();
        let mut locked_at = None;
        let mut worst_after = 0f64;
        run(cfg, 120, |i, _, g| {
            let st = g.status();
            if st.state == GenlockState::Locked && locked_at.is_none() {
                locked_at = Some(i / 30);
            }
            if i > 60 * 30 {
                worst_after = worst_after.max(st.phase_error_ns.abs());
            }
        });
        let locked_at = locked_at.expect("never locked");
        assert!(locked_at <= 20, "locked after {locked_at} s");
        // The 7.4 us duration steps are coarser than the 2.5 us per frame
        // the loop needs, so it dithers between steps: about a step times
        // the window of phase wander.
        assert!(
            worst_after < 300_000.0,
            "window error {worst_after} ns after lock"
        );
    }

    #[test]
    fn follows_a_thermal_rate_change() {
        let cfg = GenlockConfig::default();
        let mut worst = 0f64;
        let mut correction = 0.0;
        // Rate error walks from -75 to +40 ppm over five minutes.
        // Without line steps, so the integral can be checked exactly; the
        // other tests cover the steps.
        run(cfg, 420, |i, s, g| {
            s.step_ns = 1.0;
            let sec = f64::from(i) / 30.0;
            if sec > 60.0 {
                s.bias_ppm = -75.0 + 115.0 * ((sec - 60.0) / 300.0).min(1.0);
                let st = g.status();
                assert_eq!(st.state, GenlockState::Locked, "lost lock at {sec:.0} s");
                worst = worst.max(st.phase_error_ns.abs());
                correction = st.correction_ppm;
            }
        });
        assert!(worst < 300_000.0, "window error {worst} ns while drifting");
        // The integral now cancels the +40 ppm sensor.
        assert!(
            (correction + 40.0).abs() < 10.0,
            "correction {correction} ppm"
        );
    }

    #[test]
    fn unsynced_clock_holds_the_rate() {
        let mut g = Genlock::new(GenlockConfig::default());
        let mut s = Sensor::new(T0 + 5_000_000.0, g.config().period_ns());
        for _ in 0..90 * 30 {
            let t = s.next();
            if let Some(d) = g.on_frame(t, true) {
                s.ask(d);
            }
        }
        let learned = g.status().correction_ppm;
        let t = s.next();
        let d = g.on_frame(t, false).expect("free run publishes once");
        assert_eq!(g.status().state, GenlockState::FreeRun);
        assert!((g.status().correction_ppm - learned).abs() < 20.0);
        assert!((d - g.config().period_ns()).abs() < 0.01 * g.config().period_ns());
        assert!(g.on_frame(s.next(), false).is_none());
    }

    #[test]
    fn locks_by_lengthening_when_frames_cannot_be_shorter() {
        // A 30 fps maximum format: no frame shorter than nominal. The sensor
        // runs fast, as measured, and starts 10 ms late. Its shortest frame
        // is a whole number of lines; with 7.4 us lines it would be 4504.5
        // and round up 110 ppm, slower than the grid, which no trim holds.
        let period = 1e9 / 30.0;
        let cfg = GenlockConfig {
            min_frame_duration_ns: period,
            ..Default::default()
        };
        let mut g = Genlock::new(cfg);
        let mut s = Sensor::new(T0 + 10_000_000.0, period);
        s.step_ns = lines_of(period);
        let mut locked = None;
        for i in 0..60 * 30 {
            let t = s.next();
            if let Some(d) = g.on_frame(t, true) {
                assert!(d >= period);
                s.ask(d);
            }
            if i % 30 == 29 && std::env::var("GL_TRACE").is_ok() {
                let st = g.status();
                eprintln!(
                    "{} {:?} e {:.0} us c {:.1} ppm",
                    i / 30,
                    st.state,
                    st.phase_error_ns / 1e3,
                    st.correction_ppm
                );
            }
            if locked.is_none() && g.status().state == GenlockState::Locked {
                locked = Some(i / 30);
            }
        }
        let locked = locked.expect("never locked");
        assert!(locked <= 25, "locked after {locked} s");
        assert!(g.status().phase_error_ns.abs() < 300_000.0);
    }

    #[test]
    fn locks_a_fast_sensor_without_slipping_when_frames_cannot_be_shorter() {
        // Measured on an iPad 6 at 1080p30: no frame shorter than nominal,
        // and the sensor runs about 1600 ppm fast against TAI. The loop
        // learns the lengthening and must trim from there, not slip a whole
        // frame each time the phase drifts a little late (it cycled every
        // 6 s between -16 and +7 ms and never locked).
        let period = 1e9 / 30.0;
        let cfg = GenlockConfig {
            min_frame_duration_ns: period,
            ..Default::default()
        };
        let mut g = Genlock::new(cfg);
        let mut s = Sensor::new(T0 + 6_000_000.0, period);
        s.bias_ppm = -1_600.0;
        let mut locked = None;
        let mut worst_after = 0f64;
        for i in 0..120 * 30 {
            let t = s.next();
            if let Some(d) = g.on_frame(t, true) {
                assert!(d >= period);
                s.ask(d);
            }
            let st = g.status();
            if i % 30 == 29 && std::env::var("GL_TRACE").is_ok() {
                eprintln!(
                    "{} {:?} e {:.0} us c {:.1} ppm",
                    i / 30,
                    st.state,
                    st.phase_error_ns / 1e3,
                    st.correction_ppm
                );
            }
            if locked.is_none() && st.state == GenlockState::Locked {
                locked = Some(i / 30);
            }
            if i > 60 * 30 {
                assert_eq!(st.state, GenlockState::Locked, "lost lock at {} s", i / 30);
                worst_after = worst_after.max(st.phase_error_ns.abs());
            }
        }
        let locked = locked.expect("never locked");
        assert!(locked <= 40, "locked after {locked} s");
        assert!(
            worst_after < 500_000.0,
            "window error {worst_after} ns after lock"
        );
    }

    #[test]
    fn relocks_after_a_step_when_frames_cannot_be_shorter() {
        // The iPad 6 again: locked, then the phase jumped 3 ms late (a clock
        // step). It must slip or trim back and lock again, not wind its rate
        // down against the shortest frame and stay late.
        let period = 1e9 / 30.0;
        let cfg = GenlockConfig {
            min_frame_duration_ns: period,
            ..Default::default()
        };
        let mut g = Genlock::new(cfg);
        let mut s = Sensor::new(T0 + 6_000_000.0, period);
        s.bias_ppm = -300.0;
        let mut relocked = None;
        for i in 0..150 * 30 {
            if i == 60 * 30 {
                assert_eq!(g.status().state, GenlockState::Locked);
                s.t += 3_000_000.0;
            }
            let t = s.next();
            if let Some(d) = g.on_frame(t, true) {
                assert!(d >= period);
                s.ask(d);
            }
            let st = g.status();
            if std::env::var("GL_TRACE").is_ok() && i % 30 == 29 {
                eprintln!(
                    "{} {:?} e {:.0} us c {:.1} ppm",
                    i / 30,
                    st.state,
                    st.phase_error_ns / 1e3,
                    st.correction_ppm
                );
            }
            if i > 62 * 30 && relocked.is_none() && st.state == GenlockState::Locked {
                relocked = Some(i / 30);
            }
        }
        let relocked = relocked.expect("never locked again");
        assert!(relocked <= 120, "locked again after {relocked} s");
        assert_eq!(g.status().state, GenlockState::Locked);
        assert!(
            g.status().correction_ppm >= -1.0,
            "rate wound below nominal"
        );
    }

    #[test]
    fn trims_a_small_late_step_when_frames_cannot_be_shorter() {
        // Locked on the minimum-is-nominal format with the measured -75 ppm
        // sensor, then the phase jumps 0.7 ms late. The learned lengthening
        // gives back 2.5 us a frame, so the trim takes about ten windows. It
        // used to unlearn that against the minimum and slip a frame instead.
        let period = 1e9 / 30.0;
        let cfg = GenlockConfig {
            min_frame_duration_ns: period,
            ..Default::default()
        };
        let mut g = Genlock::new(cfg);
        let mut s = Sensor::new(T0 + 6_000_000.0, period);
        s.step_ns = lines_of(period);
        tally(&mut g, &mut s, 60, |_, _, _| {});
        assert_eq!(g.status().state, GenlockState::Locked);
        let mut back = None;
        let after = tally(&mut g, &mut s, 60, |i, s, g| {
            if i == 0 {
                s.t += 700_000.0;
            }
            if i > 30 && back.is_none() && g.status().phase_error_ns.abs() < 150_000.0 {
                back = Some(i / 30);
            }
        });
        assert!(after.frame_slips.is_empty(), "{after:?}");
        assert_eq!(after.slip_windows, 0, "{after:?}");
        let back = back.expect("never trimmed back");
        assert!(back <= 15, "trimmed back after {back} s");
        assert_eq!(after.locked_windows, after.windows, "{after:?}");
        assert!(!g.status().format_too_slow);
    }

    #[test]
    fn trims_a_late_step_with_the_learned_headroom() {
        // The iPad 6 (1600 ppm fast) on its minimum-is-nominal format, then
        // 3 ms late. Nominal leaves no headroom, but the learned lengthening
        // leaves 53 us a frame: trim within two windows, no slip.
        let period = 1e9 / 30.0;
        let cfg = GenlockConfig {
            min_frame_duration_ns: period,
            ..Default::default()
        };
        let mut g = Genlock::new(cfg);
        let mut s = Sensor::new(T0 + 6_000_000.0, period);
        s.bias_ppm = -1_600.0;
        tally(&mut g, &mut s, 60, |_, _, _| {});
        assert_eq!(g.status().state, GenlockState::Locked);
        let after = tally(&mut g, &mut s, 60, |i, s, _| {
            if i == 0 {
                s.t += 3_000_000.0;
            }
        });
        assert!(after.frame_slips.is_empty(), "{after:?}");
        assert_eq!(after.slip_windows, 0, "{after:?}");
        assert_eq!(g.status().state, GenlockState::Locked);
    }

    #[test]
    fn drifts_instead_of_slipping_when_the_sensor_is_too_slow_for_the_format() {
        // A sensor 75 ppm slow on a format with no frame shorter than
        // nominal: frames at the minimum land 2.5 us late each, and nothing
        // the loop can ask for stops that. The phase must drift late, wrap at
        // half a period (a whole frame against the grid) and be pulled back
        // from the early side: one frame every half period / 75 ppm, 222 s
        // plus the pull. It used to slip at 0.5 ms late, every 13 s, and
        // reported Locked up to 1.9 ms off. Run with 7.4 us lines too, which
        // make the shortest frame 185 ppm slow.
        let period = 1e9 / 30.0;
        let cfg = GenlockConfig {
            min_frame_duration_ns: period,
            ..Default::default()
        };
        for step_ns in [lines_of(period), 7_400.0] {
            let mut g = Genlock::new(cfg);
            let mut s = Sensor::new(T0 + 6_000_000.0, period);
            s.bias_ppm = 75.0;
            s.step_ns = step_ns;
            let shortest = (period / step_ns).round() * step_ns * (1.0 + 75e-6);
            let wrap_s = period / 2.0 / (shortest - period) * period / 1e9;
            // The learned rate never goes below the minimum.
            let floor = |_: u32, _: &mut Sensor, g: &Genlock| {
                assert!(g.integral >= -1e-9, "integral {}", g.integral);
            };
            // One slip while acquiring, before anything is known; the verdict
            // takes a few windows of drift after the lock.
            let start = tally(&mut g, &mut s, 30, floor);
            assert!(start.frame_slips.len() == 1 && start.slip_windows <= 3);
            assert!(g.status().format_too_slow, "{step_ns}: {start:?}");
            let run = tally(&mut g, &mut s, 1800, floor);
            assert_eq!(run.slip_windows, 0, "{step_ns}: {run:?}");
            assert!(run.frame_slips.len() >= 3, "{step_ns}: {run:?}");
            for w in run.frame_slips.windows(2) {
                let gap = w[1] - w[0];
                assert!(
                    gap > wrap_s && gap < wrap_s + 20.0,
                    "{step_ns}: a frame after {gap:.0} s, expected {wrap_s:.0} s"
                );
            }
            // Locked only while the phase really is on the grid.
            assert!(
                run.worst_locked_ns < cfg.lock_threshold_ns,
                "{step_ns}: {run:?}"
            );
            assert!(g.status().format_too_slow);
        }
    }

    #[test]
    fn locks_again_when_a_too_slow_sensor_speeds_up() {
        // Too slow for the format at first (+75 ppm), then the sensor runs
        // 75 ppm fast, at several points of the drift. Frames at the minimum
        // catch up again, the verdict clears, and the loop slips at most
        // once (it unlearned its rate while too slow) and holds the grid.
        // The slip must not teach the rate: its first window follows a slow
        // drift, so the phase looks held, and learning there asks for
        // frames 1600 ppm long and keeps it off the grid for minutes.
        let period = 1e9 / 30.0;
        let cfg = GenlockConfig {
            min_frame_duration_ns: period,
            ..Default::default()
        };
        for at in [240, 280, 320, 360] {
            let mut g = Genlock::new(cfg);
            let mut s = Sensor::new(T0 + 6_000_000.0, period);
            s.step_ns = lines_of(period);
            s.bias_ppm = 75.0;
            tally(&mut g, &mut s, at, |_, _, _| {});
            assert!(g.status().format_too_slow);
            s.bias_ppm = -75.0;
            let mut locked = None;
            let first = tally(&mut g, &mut s, 60, |i, _, g| {
                if locked.is_none() && g.status().state == GenlockState::Locked {
                    locked = Some(i / 30);
                }
            });
            assert!(first.frame_slips.len() <= 1, "{at}: {first:?}");
            let locked = locked.expect("never locked again");
            assert!(locked <= 30, "{at}: locked again after {locked} s");
            assert!(!g.status().format_too_slow);
            let held = tally(&mut g, &mut s, 60, |_, _, _| {});
            assert!(held.frame_slips.is_empty(), "{at}: {held:?}");
            assert_eq!(held.locked_windows, held.windows, "{at}: {held:?}");
        }
    }

    #[test]
    fn follows_a_new_target_offset() {
        let cfg = GenlockConfig::default();
        let mut g = Genlock::new(cfg);
        let mut s = Sensor::new(T0 + 4_000_000.0, cfg.period_ns());
        let feed = |g: &mut Genlock, s: &mut Sensor, frames: u32| {
            for _ in 0..frames {
                let t = s.next();
                if let Some(d) = g.on_frame(t, true) {
                    s.ask(d);
                }
            }
        };
        feed(&mut g, &mut s, 40 * 30);
        assert_eq!(g.status().state, GenlockState::Locked);
        g.set_target_offset_ns(5_000_000);
        feed(&mut g, &mut s, 20 * 30);
        let st = g.status();
        assert_eq!(st.state, GenlockState::Locked);
        assert!(st.phase_error_ns.abs() < 300_000.0, "{st:?}");
        // The frames now land 5 ms after the grid points.
        let t = s.next();
        let e = Genlock::new(cfg).phase_error_ns(t);
        assert!((e - 5_000_000.0).abs() < 400_000.0, "{e}");
    }

    #[test]
    fn never_asks_below_the_format_minimum() {
        let cfg = GenlockConfig {
            fps_num: 60,
            min_frame_duration_ns: 1e9 / 60.0,
            ..Default::default()
        };
        run(cfg, 20, |_, _, g| {
            assert!(g.frame_duration_ns() >= 1e9 / 60.0);
        });
    }
}
