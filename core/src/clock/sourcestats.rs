//! Per-source sample register and regression, ported from chrony's
//! `sourcestats.c`.
//!
//! Sign convention inside this module is chrony's: an offset is positive
//! when the local clock is FAST of the source; a frequency is the rate at
//! which the local clock gains on the source. The "local clock" here is the
//! raw host timescale `R(h)` (host ns plus a constant), which the discipline
//! never adjusts, so samples never need to be slewed.

use super::secs;
use std::collections::VecDeque;

use super::regress::{self, MIN_SAMPLES_FOR_REGRESS, REGRESS_RUNS_RATIO};

pub const MAX_SAMPLES: usize = 64;
/// Assumed worst case bound on an unknown frequency.
pub const WORST_CASE_FREQ_BOUND: f64 = 2000.0 / 1.0e6;
const MIN_SKEW: f64 = 1.0e-12;
const MAX_SKEW: f64 = 1.0;
const MIN_STDDEV: f64 = 1.0e-9;
const WORST_CASE_STDDEV_BOUND: f64 = 4.0;
const MAX_ASYMMETRY: f64 = 0.5;
const MIN_ASYMMETRY: f64 = 0.45;
const MIN_ASYMMETRY_RUN: i32 = 10;
const MAX_ASYMMETRY_RUN: i32 = 1000;
/// Assumed ratio of the offset standard deviation to the spread of the peer
/// distances.
const SD_TO_DIST_RATIO: f64 = 0.7;

/// One measurement, as accumulated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Host ns at the middle of the exchange.
    pub time: i64,
    /// Seconds, positive = local fast (slewed by `correct_offset`).
    pub offset: f64,
    /// As measured.
    pub orig_offset: f64,
    pub peer_delay: f64,
    pub peer_dispersion: f64,
    pub root_delay: f64,
    pub root_dispersion: f64,
}

#[derive(Debug, Clone, Copy)]
struct RunSample {
    time: i64,
    offset: f64,
    peer_delay: f64,
}

/// What source selection needs from a source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SelectionData {
    /// Offset now, positive = local fast.
    pub offset: f64,
    pub lo_limit: f64,
    pub hi_limit: f64,
    pub root_distance: f64,
    pub std_dev: f64,
    pub first_sample_ago: f64,
    pub last_sample_ago: f64,
}

/// What the reference update needs from a source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackingData {
    /// Host ns the offset refers to.
    pub ref_time: i64,
    pub offset: f64,
    pub offset_sd: f64,
    pub frequency: f64,
    pub frequency_sd: f64,
    pub skew: f64,
    pub root_delay: f64,
    pub root_dispersion: f64,
}

#[derive(Debug, Clone)]
pub struct SourceStats {
    samples: VecDeque<Sample>,
    /// Older samples kept only to extend the runs test (oldest first).
    runs: VecDeque<RunSample>,
    max_samples: usize,
    min_samples: usize,
    precision: f64,
    regression_ok: bool,
    best_single_sample: usize,
    estimated_offset: f64,
    estimated_offset_sd: f64,
    offset_time: i64,
    nruns: usize,
    asymmetry_run: i32,
    asymmetry: f64,
    estimated_frequency: f64,
    estimated_frequency_sd: f64,
    skew: f64,
    std_dev: f64,
    /// Samples dropped by the last regression (statistics log).
    last_best_start: usize,
}

impl SourceStats {
    pub fn new(precision: f64) -> Self {
        let mut s = SourceStats {
            samples: VecDeque::with_capacity(MAX_SAMPLES),
            runs: VecDeque::with_capacity(MAX_SAMPLES),
            max_samples: MAX_SAMPLES,
            min_samples: 1,
            precision,
            regression_ok: false,
            best_single_sample: 0,
            estimated_offset: 0.0,
            estimated_offset_sd: 0.0,
            offset_time: 0,
            nruns: 0,
            asymmetry_run: 0,
            asymmetry: 0.0,
            estimated_frequency: 0.0,
            estimated_frequency_sd: 0.0,
            skew: 0.0,
            std_dev: 0.0,
            last_best_start: 0,
        };
        s.reset();
        s
    }

    pub fn reset(&mut self) {
        self.samples.clear();
        self.runs.clear();
        self.regression_ok = false;
        self.best_single_sample = 0;
        self.estimated_frequency = 0.0;
        self.estimated_frequency_sd = WORST_CASE_FREQ_BOUND;
        self.skew = WORST_CASE_FREQ_BOUND;
        self.estimated_offset = 0.0;
        self.estimated_offset_sd = WORST_CASE_STDDEV_BOUND;
        self.offset_time = 0;
        self.std_dev = WORST_CASE_STDDEV_BOUND;
        self.nruns = 0;
        self.asymmetry_run = 0;
        self.asymmetry = 0.0;
    }

    pub fn n_samples(&self) -> usize {
        self.samples.len()
    }

    pub fn n_runs(&self) -> usize {
        self.nruns
    }

    pub fn regression_ok(&self) -> bool {
        self.regression_ok
    }

    pub fn std_dev(&self) -> f64 {
        self.std_dev
    }

    pub fn skew(&self) -> f64 {
        self.skew
    }

    pub fn frequency(&self) -> f64 {
        self.estimated_frequency
    }

    pub fn asymmetry(&self) -> f64 {
        self.asymmetry
    }

    pub fn last_best_start(&self) -> usize {
        self.last_best_start
    }

    pub fn samples(&self) -> impl Iterator<Item = &Sample> {
        self.samples.iter()
    }

    /// Moves the `k` oldest samples to the runs-test history.
    fn prune(&mut self, k: usize) {
        for _ in 0..k {
            if let Some(s) = self.samples.pop_front() {
                self.runs.push_back(RunSample {
                    time: s.time,
                    offset: s.offset,
                    peer_delay: s.peer_delay,
                });
            }
        }
        let max_runs = self.samples.len() * (REGRESS_RUNS_RATIO - 1);
        while self.runs.len() > max_runs {
            self.runs.pop_front();
        }
    }

    pub fn accumulate(&mut self, sample: Sample) {
        if !self.samples.is_empty() && self.samples.len() >= self.max_samples {
            self.prune(1);
        }
        if let Some(last) = self.samples.back() {
            if last.time >= sample.time {
                // Out of order (cannot happen with a monotonic host clock).
                self.reset();
            }
        }
        self.samples.push_back(sample);
    }

    /// Minimum round-trip delay over the register and the runs history.
    pub fn min_round_trip_delay(&self) -> f64 {
        let a = self.samples.iter().map(|s| s.peer_delay);
        let b = self.runs.iter().map(|s| s.peer_delay);
        a.chain(b).fold(f64::MAX, f64::min)
    }

    /// Re-runs the regression after a new sample (chrony
    /// `SST_DoNewRegression`).
    pub fn do_new_regression(&mut self) {
        let n = self.samples.len();
        let m = self.runs.len();
        let last_time = match self.samples.back() {
            Some(s) => s.time,
            None => {
                self.regression_ok = false;
                return;
            }
        };
        let mut x = Vec::with_capacity(m + n);
        let mut y = Vec::with_capacity(m + n);
        let mut delays = Vec::with_capacity(m + n);
        for r in &self.runs {
            x.push(secs(r.time - last_time));
            y.push(r.offset);
            delays.push(r.peer_delay);
        }
        for s in &self.samples {
            x.push(secs(s.time - last_time));
            y.push(s.offset);
            delays.push(s.peer_delay);
        }

        // Weights from the peer distances: samples with more delay than the
        // minimum count less.
        let peer_distances: Vec<f64> = self
            .samples
            .iter()
            .map(|s| 0.5 * s.peer_delay + s.peer_dispersion)
            .collect();
        let mut min_distance = peer_distances.iter().copied().fold(f64::MAX, f64::min);
        let median_distance = regress::median(&peer_distances);
        let mut sd = (median_distance - min_distance) / SD_TO_DIST_RATIO;
        sd = sd.max(self.precision).min(min_distance.max(self.precision));
        min_distance += self.precision;
        let weights: Vec<f64> = peer_distances
            .iter()
            .map(|&d| {
                let mut w = 1.0;
                if d > min_distance {
                    w += (d - min_distance) / sd;
                }
                w * w
            })
            .collect();

        self.correct_asymmetry(&x, &mut y, &delays);

        match regress::find_best_regression(&x, &y, &weights, m, self.min_samples) {
            Some(r) => {
                self.estimated_frequency = r.b1;
                self.estimated_frequency_sd = r.sb1.clamp(MIN_SKEW, MAX_SKEW);
                self.skew = (r.sb1 * regress::t_coef(r.dof)).clamp(MIN_SKEW, MAX_SKEW);
                self.estimated_offset = r.b0;
                self.offset_time = last_time;
                self.estimated_offset_sd = r.sb0;
                self.std_dev = r.s2.sqrt().max(MIN_STDDEV);
                self.nruns = r.n_runs;
                self.regression_ok = true;
                self.last_best_start = r.new_start;
                self.prune(r.new_start);
            }
            None => {
                self.regression_ok = false;
                self.estimated_frequency_sd = WORST_CASE_FREQ_BOUND;
                self.skew = WORST_CASE_FREQ_BOUND;
                self.estimated_offset_sd = WORST_CASE_STDDEV_BOUND;
                self.std_dev = WORST_CASE_STDDEV_BOUND;
                self.nruns = 0;
                self.last_best_start = 0;
                let last = self.samples.back().copied();
                self.estimated_offset = last.map_or(0.0, |s| s.offset);
                self.offset_time = last.map_or(0, |s| s.time);
            }
        }
        self.find_best_sample_index();
    }

    /// Estimates the asymmetry of the network jitter as the slope of offset
    /// against delay and, when it is significant and stable, corrects the
    /// offsets towards the minimum-delay ones (chrony default `asymmetry`
    /// estimation).
    fn correct_asymmetry(&mut self, x: &[f64], y: &mut [f64], delays: &[f64]) {
        let min_delay = self.min_round_trip_delay();
        let d: Vec<f64> = delays.iter().map(|v| v - min_delay).collect();
        let a = match regress::multiple_regress_b2(x, &d, y) {
            Some(a) if a * self.asymmetry_run as f64 >= 0.0 => a,
            _ => {
                self.asymmetry = 0.0;
                self.asymmetry_run = 0;
                return;
            }
        };
        if a <= -MIN_ASYMMETRY && self.asymmetry_run > -MAX_ASYMMETRY_RUN {
            self.asymmetry_run -= 1;
        } else if a >= MIN_ASYMMETRY && self.asymmetry_run < MAX_ASYMMETRY_RUN {
            self.asymmetry_run += 1;
        }
        if self.asymmetry_run.abs() < MIN_ASYMMETRY_RUN {
            return;
        }
        self.asymmetry = a.clamp(-MAX_ASYMMETRY, MAX_ASYMMETRY);
        for (yi, di) in y.iter_mut().zip(&d) {
            *yi -= self.asymmetry * di;
        }
    }

    /// The sample with the smallest root distance now, given the skew.
    fn find_best_sample_index(&mut self) {
        let Some(last) = self.samples.back() else {
            return;
        };
        let last_time = last.time;
        let mut best = (usize::MAX, f64::MAX);
        for (i, s) in self.samples.iter().enumerate() {
            let elapsed = secs(last_time - s.time);
            let d = s.root_dispersion + elapsed * self.skew + 0.5 * s.root_delay;
            if d < best.1 {
                best = (i, d);
            }
        }
        self.best_single_sample = best.0;
    }

    /// Worst case range of the frequency, clamped like chrony for the
    /// dispersion of new samples.
    pub fn frequency_range(&self) -> (f64, f64) {
        if self.skew > WORST_CASE_FREQ_BOUND {
            return (-WORST_CASE_FREQ_BOUND, WORST_CASE_FREQ_BOUND);
        }
        (
            self.estimated_frequency - self.skew,
            self.estimated_frequency + self.skew,
        )
    }

    /// Interval for source selection at host time `now`; `None` without a
    /// successful regression.
    pub fn selection_data(&self, now: i64) -> Option<SelectionData> {
        let best = self.samples.get(self.best_single_sample)?;
        let first = self.samples.front()?;
        let last = self.samples.back()?;
        let sample_elapsed = secs(now - best.time).abs();
        let offset = best.offset + sample_elapsed * self.estimated_frequency;
        let root_distance =
            0.5 * best.root_delay + best.root_dispersion + sample_elapsed * self.skew;
        if !self.regression_ok {
            return None;
        }
        Some(SelectionData {
            offset,
            lo_limit: offset - root_distance,
            hi_limit: offset + root_distance,
            root_distance,
            std_dev: self.std_dev,
            first_sample_ago: secs(now - first.time),
            last_sample_ago: secs(now - last.time),
        })
    }

    pub fn tracking_data(&self) -> Option<TrackingData> {
        let best = self.samples.get(self.best_single_sample)?;
        let elapsed_sample = secs(self.offset_time - best.time);
        Some(TrackingData {
            ref_time: self.offset_time,
            offset: self.estimated_offset,
            offset_sd: self.estimated_offset_sd,
            frequency: self.estimated_frequency,
            frequency_sd: self.estimated_frequency_sd,
            skew: self.skew,
            root_delay: best.root_delay,
            root_dispersion: best.root_dispersion
                + self.skew * elapsed_sample
                + self.estimated_offset_sd,
        })
    }

    /// Offset predicted at host time `when` (positive = local fast).
    pub fn predict_offset(&self, when: i64) -> f64 {
        if self.samples.len() < MIN_SAMPLES_FOR_REGRESS {
            return self.samples.back().map_or(0.0, |s| s.offset);
        }
        self.estimated_offset + secs(when - self.offset_time) * self.estimated_frequency
    }

    /// (last_sample_ago, predicted_offset, min_delay, skew, std_dev) for the
    /// delay tests; `None` with fewer than 6 samples.
    pub fn delay_test_data(&self, sample_time: i64) -> Option<(f64, f64, f64, f64, f64)> {
        if self.samples.len() < 6 {
            return None;
        }
        let ago = secs(sample_time - self.offset_time);
        Some((
            ago,
            self.estimated_offset + ago * self.estimated_frequency,
            self.min_round_trip_delay(),
            self.skew,
            self.std_dev,
        ))
    }

    /// Shifts every offset, e.g. when the raw timescale is rebased.
    pub fn correct_offset(&mut self, doffset: f64) {
        for s in self.samples.iter_mut() {
            s.offset += doffset;
        }
        for r in self.runs.iter_mut() {
            r.offset += doffset;
        }
        self.estimated_offset += doffset;
    }

    /// `chronyc sourcestats` row: (span s, est offset, est offset error,
    /// residual frequency, skew, std dev).
    pub fn sourcestats(&self, now: i64) -> (f64, f64, f64, f64, f64, f64) {
        let (Some(first), Some(last), Some(best)) = (
            self.samples.front(),
            self.samples.back(),
            self.samples.get(self.best_single_sample),
        ) else {
            return (
                0.0,
                0.0,
                0.0,
                self.estimated_frequency,
                self.skew,
                self.std_dev,
            );
        };
        let span = secs(last.time - first.time);
        let elapsed = secs(now - self.offset_time);
        let sample_elapsed = secs(now - best.time);
        (
            span,
            self.estimated_offset + elapsed * self.estimated_frequency,
            self.estimated_offset_sd
                + sample_elapsed * self.skew
                + 0.5 * best.root_delay
                + best.root_dispersion,
            self.estimated_frequency,
            self.skew,
            self.std_dev,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t_s: f64, offset: f64, delay: f64) -> Sample {
        Sample {
            time: (t_s * 1e9) as i64,
            offset,
            orig_offset: offset,
            peer_delay: delay,
            peer_dispersion: 1e-6,
            root_delay: delay + 0.01,
            root_dispersion: 0.001,
        }
    }

    #[test]
    fn tracks_offset_and_frequency() {
        let mut s = SourceStats::new(1e-7);
        for i in 0..20 {
            let t = 1000.0 + i as f64 * 64.0;
            s.accumulate(sample(t, 0.005 + 20e-6 * (t - 1000.0), 0.02));
            s.do_new_regression();
        }
        assert!(s.regression_ok());
        let td = s.tracking_data().unwrap();
        assert!((td.frequency - 20e-6).abs() < 1e-9);
        let now = ((1000.0 + 19.0 * 64.0 + 10.0) * 1e9) as i64;
        assert!((s.predict_offset(now) - (0.005 + 20e-6 * (19.0 * 64.0 + 10.0))).abs() < 1e-7);
        let sel = s.selection_data(now).unwrap();
        assert!(sel.lo_limit < sel.offset && sel.offset < sel.hi_limit);
    }

    #[test]
    fn needs_three_samples() {
        let mut s = SourceStats::new(1e-7);
        s.accumulate(sample(1.0, 0.0, 0.01));
        s.do_new_regression();
        s.accumulate(sample(3.0, 0.0, 0.01));
        s.do_new_regression();
        assert!(s.selection_data(4_000_000_000).is_none());
        s.accumulate(sample(5.0, 0.0, 0.01));
        s.do_new_regression();
        assert!(s.selection_data(6_000_000_000).is_some());
    }

    #[test]
    fn register_is_bounded() {
        let mut s = SourceStats::new(1e-7);
        for i in 0..200 {
            s.accumulate(sample(i as f64 * 64.0, 0.0, 0.01 + (i % 3) as f64 * 1e-4));
            s.do_new_regression();
        }
        assert!(s.n_samples() <= MAX_SAMPLES);
        assert!(s.runs.len() <= s.n_samples());
    }
}
