//! Frame timing against the TAI grid: what a soft genlock has to correct.
//!
//! Every stamped frame gives (frame counter, capture TAI). A least-squares
//! line through capture time over frame index gives the real frame period;
//! its distance from the nominal period is the camera's rate error in ppm,
//! which moves the frame phase against the TAI grid at ppm * 0.06 ms per
//! minute. The residuals around the line are the capture jitter a phase
//! regulator would see.
//!
//! A stream restart (the frame counter starts over, or capture time jumps)
//! starts a new segment. The period is fitted across all segments with a
//! separate offset each, so restarts do not bend the line.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result};

/// One stamped frame.
#[derive(Clone, Copy)]
struct Frame {
    /// Frame index since the segment's first frame (counter gaps included).
    index: i64,
    capture_ns: i64,
}

/// A capture gap longer than this starts a new segment.
const SEGMENT_GAP_NS: i64 = 1_000_000_000;

#[derive(Default)]
pub struct FrameTiming {
    /// Frames of each run without a restart.
    segments: Vec<Vec<Frame>>,
    last_counter: Option<u32>,
    gaps: u64,
    csv: Option<BufWriter<File>>,
}

impl FrameTiming {
    /// Also writes one CSV row per frame to `path`.
    pub fn with_csv(path: &Path) -> Result<Self> {
        let mut w = BufWriter::new(
            File::create(path).with_context(|| format!("create {}", path.display()))?,
        );
        writeln!(
            w,
            "frame_counter,capture_tai_ns,arrival_tai_ns,uncertainty_us,synced"
        )?;
        Ok(Self {
            csv: Some(w),
            ..Default::default()
        })
    }

    pub fn push(
        &mut self,
        counter: u32,
        capture_ns: i64,
        arrival_ns: i64,
        uncertainty_us: Option<u32>,
        synced: bool,
    ) {
        let prev = self.segments.last().and_then(|s| s.last()).copied();
        let next = match (self.last_counter, prev) {
            (Some(last), Some(prev)) => {
                let step = counter.wrapping_sub(last);
                let continuous = (1..1000).contains(&step)
                    && capture_ns > prev.capture_ns
                    && capture_ns - prev.capture_ns < SEGMENT_GAP_NS;
                if continuous {
                    if step > 1 {
                        self.gaps += 1;
                    }
                    Some(prev.index + i64::from(step))
                } else {
                    None
                }
            }
            _ => None,
        };
        self.last_counter = Some(counter);
        match next {
            Some(index) => self
                .segments
                .last_mut()
                .expect("a previous frame")
                .push(Frame { index, capture_ns }),
            None => self.segments.push(vec![Frame {
                index: 0,
                capture_ns,
            }]),
        }
        if let Some(w) = &mut self.csv {
            let unc = uncertainty_us.map_or(String::new(), |u| u.to_string());
            let _ = writeln!(w, "{counter},{capture_ns},{arrival_ns},{unc},{synced}");
        }
    }

    /// Reads a CSV written by `with_csv`.
    pub fn from_csv(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let mut t = Self::default();
        for (i, line) in text.lines().enumerate().skip(1) {
            let cols: Vec<&str> = line.split(',').collect();
            let field = |k: usize| cols.get(k).copied().unwrap_or_default();
            let parsed = (
                field(0).parse::<u32>(),
                field(1).parse::<i64>(),
                field(2).parse::<i64>(),
            );
            let (Ok(counter), Ok(capture), Ok(arrival)) = parsed else {
                anyhow::bail!("{}:{}: bad row '{line}'", path.display(), i + 1);
            };
            t.push(
                counter,
                capture,
                arrival,
                field(3).parse().ok(),
                field(4) == "true",
            );
        }
        Ok(t)
    }

    pub fn flush(&mut self) {
        if let Some(w) = &mut self.csv {
            let _ = w.flush();
        }
    }

    /// Summary lines, or None with fewer than two seconds of frames.
    pub fn report(&self) -> Option<Vec<String>> {
        // Segments shorter than a second say little about the period.
        let segments: Vec<&Vec<Frame>> = self.segments.iter().filter(|s| s.len() >= 30).collect();
        let n: usize = segments.iter().map(|s| s.len()).sum();
        if n < 60 {
            return None;
        }
        // Pooled least squares: one slope (the period), one offset per
        // segment. Coordinates are relative to each segment's first frame
        // (30 min at 60 fps is ~1.8e12 ns, exact in f64).
        let rel = |seg: &[Frame], f: &Frame| {
            (
                (f.index - seg[0].index) as f64,
                (f.capture_ns - seg[0].capture_ns) as f64,
            )
        };
        let (mut sxy, mut sxx) = (0.0, 0.0);
        let mut means = Vec::with_capacity(segments.len());
        for seg in &segments {
            let k = seg.len() as f64;
            let (mx, my) = seg.iter().fold((0.0, 0.0), |(ax, ay), f| {
                let (x, y) = rel(seg, f);
                (ax + x / k, ay + y / k)
            });
            for f in seg.iter() {
                let (x, y) = rel(seg, f);
                sxy += (x - mx) * (y - my);
                sxx += (x - mx) * (x - mx);
            }
            means.push((mx, my));
        }
        let period = sxy / sxx;
        let fps = (1e9 / period).round();
        let nominal = 1e9 / fps;
        let ppm = (period - nominal) / nominal * 1e6;

        let mut residual_us = Vec::with_capacity(n);
        let mut glitches = 0usize;
        for (seg, (mx, my)) in segments.iter().zip(&means) {
            for f in seg.iter() {
                let (x, y) = rel(seg, f);
                residual_us.push(((y - my - period * (x - mx)) / 1e3).abs());
            }
            // Intervals more than a quarter frame off the fitted period.
            glitches += seg
                .windows(2)
                .filter(|w| {
                    let frames = (w[1].index - w[0].index) as f64;
                    let dt = (w[1].capture_ns - w[0].capture_ns) as f64;
                    (dt - frames * period).abs() > period / 4.0
                })
                .count();
        }
        residual_us.sort_by(f64::total_cmp);
        let pct = |p: f64| residual_us[((residual_us.len() - 1) as f64 * p).round() as usize];

        let first = segments[0][0].capture_ns;
        let last_seg = segments[segments.len() - 1];
        let last = last_seg[last_seg.len() - 1].capture_ns;
        let covered_s: f64 = segments
            .iter()
            .map(|s| (s[s.len() - 1].capture_ns - s[0].capture_ns) as f64 / 1e9)
            .sum();
        let fps_i = fps as i64;
        let phase = |t: i64| crate::frame_phase_ns(t, fps_i) as f64 / 1e6;
        Some(vec![
            format!(
                "frame timing over {covered_s:.0} s in {} segment(s) ({n} frames, {} counter gaps, {glitches} intervals off by > 1/4 frame)",
                segments.len(),
                self.gaps
            ),
            format!(
                "  period {:.4} ms at {fps_i} fps nominal: rate error {ppm:+.2} ppm, phase drift {:+.3} ms/min",
                period / 1e6,
                ppm * 60.0 * 1e-6 * 1e3
            ),
            format!(
                "  phase on the TAI grid: first {:.2} ms, last {:.2} ms (period {:.2} ms)",
                phase(first),
                phase(last),
                nominal / 1e6
            ),
            format!(
                "  capture jitter around the fit, us: p50 {:.0} p95 {:.0} p99 {:.0} max {:.0}",
                pct(0.5),
                pct(0.95),
                pct(0.99),
                residual_us[residual_us.len() - 1]
            ),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_recovers_rate_error_and_counts_gaps() {
        let mut t = FrameTiming::default();
        // 30 fps running 20 ppm slow, frame 50 missing.
        let period = 1e9 / 30.0 * (1.0 + 20e-6);
        for i in 0..3000u32 {
            if i == 50 {
                continue;
            }
            t.push(
                i,
                5_000_000_000 + (f64::from(i) * period) as i64,
                0,
                None,
                true,
            );
        }
        let lines = t.report().unwrap();
        assert!(lines[0].contains("1 counter gaps"), "{}", lines[0]);
        assert!(lines[0].contains("0 intervals"), "{}", lines[0]);
        assert!(lines[1].contains("+20.00 ppm"), "{}", lines[1]);
    }

    #[test]
    fn restart_starts_a_segment_and_keeps_the_fit() {
        let mut t = FrameTiming::default();
        let period = 1e9 / 30.0 * (1.0 - 50e-6);
        // 60 s live, 9.7 s away, then the counter starts over.
        for i in 0..1800u32 {
            t.push(
                i,
                1_000_000_000 + (f64::from(i) * period) as i64,
                0,
                None,
                true,
            );
        }
        let restart = 1_000_000_000 + 69_700_000_000i64;
        for i in 0..900u32 {
            t.push(i, restart + (f64::from(i) * period) as i64, 0, None, true);
        }
        let lines = t.report().unwrap();
        assert!(lines[0].contains("2 segment(s)"), "{}", lines[0]);
        assert!(lines[0].contains("0 intervals"), "{}", lines[0]);
        assert!(lines[1].contains("-50.00 ppm"), "{}", lines[1]);
    }
}
