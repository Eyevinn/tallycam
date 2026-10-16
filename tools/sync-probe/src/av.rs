//! Flash and beep onset detection for lip-sync measurements.
//!
//! The test signal is a picture that lights up (fake-camera's flash box, or
//! a screen the phone films) and a beep that starts at the same instant.
//! Both onsets are taken on the capture timescale (TAI ns from the LCP SEI),
//! so their difference is the A/V offset the stream carries.

use std::collections::VecDeque;

const NS: i64 = 1_000_000_000;
/// Onsets of one kind closer together than this are one event.
const DEBOUNCE_NS: i64 = 400_000_000;

/// Cells per side of the brightness grid: the flash may cover only part of
/// the picture (fake-camera's box, a screen in the phone's view).
pub const GRID: usize = 8;

/// Finds frames where the picture brightness steps up, in the grid cell
/// whose brightness swings the most.
pub struct FlashDetector {
    history: VecDeque<[f64; GRID * GRID]>,
    last_onset: Option<i64>,
    /// Minimum (max - min) brightness of a cell to count as a flash.
    pub min_swing: f64,
}

impl Default for FlashDetector {
    fn default() -> Self {
        FlashDetector {
            history: VecDeque::new(),
            last_onset: None,
            min_swing: 40.0,
        }
    }
}

impl FlashDetector {
    /// `cells`: mean luma (0..255) of each grid cell, row by row;
    /// `capture_ns`: the frame's capture time. Returns the capture time of a
    /// flash onset.
    pub fn push(&mut self, cells: [f64; GRID * GRID], capture_ns: i64) -> Option<i64> {
        self.history.push_back(cells);
        if self.history.len() > 90 {
            self.history.pop_front();
        }
        if self.history.len() < 3 {
            return None;
        }
        let mut best = (0usize, 0.0f64, 0.0f64);
        for c in 0..GRID * GRID {
            let (min, max) = self
                .history
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), h| {
                    (lo.min(h[c]), hi.max(h[c]))
                });
            if max - min > best.2 - best.1 {
                best = (c, min, max);
            }
        }
        let (c, min, max) = best;
        if max - min < self.min_swing {
            return None;
        }
        let threshold = (min + max) / 2.0;
        let n = self.history.len();
        let (prev, level) = (self.history[n - 2][c], self.history[n - 1][c]);
        if prev < threshold && level >= threshold {
            if self
                .last_onset
                .is_some_and(|t| capture_ns - t < DEBOUNCE_NS)
            {
                return None;
            }
            self.last_onset = Some(capture_ns);
            return Some(capture_ns);
        }
        None
    }
}

/// Mean luma of each grid cell of a GRAY8 picture, sampling every 4th pixel.
pub fn grid_means(data: &[u8], width: usize, height: usize, stride: usize) -> [f64; GRID * GRID] {
    let mut sum = [0u64; GRID * GRID];
    let mut n = [0u64; GRID * GRID];
    for y in (0..height).step_by(4) {
        let row = &data[y * stride..y * stride + width];
        let cy = y * GRID / height;
        for x in (0..width).step_by(4) {
            let c = cy * GRID + x * GRID / width;
            sum[c] += u64::from(row[x]);
            n[c] += 1;
        }
    }
    let mut out = [0.0; GRID * GRID];
    for c in 0..GRID * GRID {
        out[c] = sum[c] as f64 / n[c].max(1) as f64;
    }
    out
}

/// A tone found by [`BeepDetector`]: onset and length, ns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Beep {
    pub onset: i64,
    pub length: i64,
}

/// Finds tones after silence in mono float samples, with their length.
pub struct BeepDetector {
    rate: u32,
    /// Absolute sample level that counts as sound.
    pub threshold: f32,
    /// Samples of silence seen in a row.
    quiet: u64,
    /// The tone being heard: onset and its last loud sample, ns.
    current: Option<(i64, i64)>,
    last_onset: Option<i64>,
}

impl BeepDetector {
    pub fn new(rate: u32) -> Self {
        BeepDetector {
            rate,
            threshold: 0.05,
            quiet: 0,
            current: None,
            last_onset: None,
        }
    }

    /// `first_ns`: capture time of `samples[0]`. Returns the tones that
    /// ended in these samples. A tone needs 200 ms of silence before it and
    /// a second loud sample within 2 ms (a click is not a beep); it ends
    /// after 10 ms of silence.
    pub fn push(&mut self, samples: &[f32], first_ns: i64) -> Vec<Beep> {
        let mut out = Vec::new();
        let rate = u64::from(self.rate);
        let need_quiet = rate / 5;
        let end_quiet = rate / 100;
        let confirm = (self.rate / 500).max(1) as usize;
        for (i, &s) in samples.iter().enumerate() {
            let t = first_ns + i as i64 * NS / i64::from(self.rate);
            if s.abs() < self.threshold {
                self.quiet += 1;
                if self.quiet == end_quiet {
                    if let Some((onset, last)) = self.current.take() {
                        out.push(Beep {
                            onset,
                            length: last - onset,
                        });
                    }
                }
                continue;
            }
            if let Some(current) = &mut self.current {
                current.1 = t;
            } else if self.quiet >= need_quiet {
                let loud_after = samples[i + 1..samples.len().min(i + 1 + confirm)]
                    .iter()
                    .filter(|x| x.abs() >= self.threshold)
                    .count();
                if loud_after > 0 && !self.last_onset.is_some_and(|last| t - last < DEBOUNCE_NS) {
                    self.last_onset = Some(t);
                    self.current = Some((t, t));
                }
            }
            self.quiet = 0;
        }
        out
    }
}

/// Pairs flash and beep onsets of the same event.
#[derive(Default)]
pub struct Pairer {
    flashes: VecDeque<i64>,
    beeps: VecDeque<i64>,
}

impl Pairer {
    pub fn flash(&mut self, t: i64) -> Option<(i64, i64)> {
        self.flashes.push_back(t);
        self.pair()
    }

    pub fn beep(&mut self, t: i64) -> Option<(i64, i64)> {
        self.beeps.push_back(t);
        self.pair()
    }

    /// (flash, beep) of one event: onsets less than half a second apart.
    fn pair(&mut self) -> Option<(i64, i64)> {
        while let (Some(&f), Some(&b)) = (self.flashes.front(), self.beeps.front()) {
            if (b - f).abs() < NS / 2 {
                self.flashes.pop_front();
                self.beeps.pop_front();
                return Some((f, b));
            }
            if f < b {
                self.flashes.pop_front();
            } else {
                self.beeps.pop_front();
            }
        }
        while self.flashes.len() > 8 {
            self.flashes.pop_front();
        }
        while self.beeps.len() > 8 {
            self.beeps.pop_front();
        }
        None
    }
}

/// Signed distance from `t` to the nearest whole second, ns.
pub fn to_whole_second(t: i64) -> i64 {
    to_nearest(t, NS)
}

/// Signed distance from `t` to the nearest multiple of `period`, ns.
pub fn to_nearest(t: i64, period: i64) -> i64 {
    let r = t.rem_euclid(period);
    if r > period / 2 {
        r - period
    } else {
        r
    }
}

/// Beeps at least this long mark every 5th second (fake-camera: 150 ms,
/// the others 50 ms).
pub const MARK_BEEP_MIN_NS: i64 = 100_000_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flash_on_rising_edge_only() {
        let mut d = FlashDetector::default();
        let period = 33_333_333;
        let mut onsets = Vec::new();
        for n in 0..120i64 {
            // A box in one corner cell lights up; the rest is a dim picture.
            let mut picture = vec![30u8; 64 * 64];
            if n % 30 == 5 {
                for y in 0..8 {
                    picture[y * 64 + 56..y * 64 + 64].fill(235);
                }
            }
            let cells = grid_means(&picture, 64, 64, 64);
            if let Some(t) = d.push(cells, n * period) {
                onsets.push(t);
            }
        }
        assert_eq!(
            onsets,
            vec![5 * period, 35 * period, 65 * period, 95 * period]
        );
    }

    #[test]
    fn beep_onset_is_sample_accurate() {
        let rate = 48_000;
        let mut d = BeepDetector::new(rate);
        // 1 s of silence, then a tone starting at sample 48_000 + 123.
        let mut samples = vec![0.0f32; 96_000];
        for (i, s) in samples.iter_mut().enumerate().skip(48_123).take(2400) {
            *s = 0.5 * ((i as f32) * 0.13).sin().signum();
        }
        let mut beeps = Vec::new();
        for (k, chunk) in samples.chunks(1024).enumerate() {
            let first = (k * 1024) as i64 * NS / i64::from(rate);
            beeps.extend(d.push(chunk, first));
        }
        assert_eq!(beeps.len(), 1);
        let expected = 48_123i64 * NS / 48_000;
        assert!(
            (beeps[0].onset - expected).abs() < 25_000,
            "{:?} vs {expected}",
            beeps[0]
        );
        // 2400 samples: 50 ms.
        assert!(
            (beeps[0].length - 50_000_000).abs() < 100_000,
            "{:?}",
            beeps[0]
        );
    }

    #[test]
    fn pairs_events() {
        let mut p = Pairer::default();
        assert_eq!(p.flash(1_000_000_000), None);
        assert_eq!(p.beep(1_030_000_000), Some((1_000_000_000, 1_030_000_000)));
        assert_eq!(p.beep(2_010_000_000), None);
        assert_eq!(p.flash(3_000_000_000), None);
        assert_eq!(p.beep(2_990_000_000), Some((3_000_000_000, 2_990_000_000)));
    }

    #[test]
    fn whole_second_distance() {
        assert_eq!(to_whole_second(5 * NS + 10), 10);
        assert_eq!(to_whole_second(5 * NS - 10), -10);
        // A beep 1.3 s after its 5-second mark is not 0.3 s after a second.
        assert_eq!(to_nearest(11 * NS + 300_000_000, 5 * NS), NS + 300_000_000);
    }
}
