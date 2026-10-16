//! Regression helpers, ported from chrony's `regress.c`.

pub const MIN_SAMPLES_FOR_REGRESS: usize = 3;
/// Extra older samples (as a multiple of the regression samples) that extend
/// the runs test.
pub const REGRESS_RUNS_RATIO: usize = 2;

/// Critical number of runs of residual signs, 5% critical region, indexed by
/// the number of residuals (chrony's table).
const CRITICAL_RUNS: [u8; 130] = [
    0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 3, 3, 4, 4, 5, 5, 5, 6, 6, 7, 7, 7, 8, 8, 9, 9, 9, 10, 10, 11,
    11, 11, 12, 12, 13, 13, 14, 14, 14, 15, 15, 16, 16, 17, 17, 18, 18, 18, 19, 19, 20, 20, 21, 21,
    21, 22, 22, 23, 23, 24, 24, 25, 25, 26, 26, 26, 27, 27, 28, 28, 29, 29, 30, 30, 30, 31, 31, 32,
    32, 33, 33, 34, 34, 35, 35, 35, 36, 36, 37, 37, 38, 38, 39, 39, 40, 40, 40, 41, 41, 42, 42, 43,
    43, 44, 44, 45, 45, 46, 46, 46, 47, 47, 48, 48, 49, 49, 50, 50, 51, 51, 52, 52, 52, 53, 53, 54,
    54, 55, 55, 56,
];

/// t-distribution 99.95% quantile for `dof` degrees of freedom.
pub fn t_coef(dof: usize) -> f64 {
    const COEFS: [f64; 40] = [
        636.6, 31.6, 12.92, 8.61, 6.869, 5.959, 5.408, 5.041, 4.781, 4.587, 4.437, 4.318, 4.221,
        4.140, 4.073, 4.015, 3.965, 3.922, 3.883, 3.850, 3.819, 3.792, 3.768, 3.745, 3.725, 3.707,
        3.690, 3.674, 3.659, 3.646, 3.633, 3.622, 3.611, 3.601, 3.591, 3.582, 3.574, 3.566, 3.558,
        3.551,
    ];
    if (1..=40).contains(&dof) {
        COEFS[dof - 1]
    } else {
        3.5
    }
}

/// Number of runs of residuals with the same sign.
pub fn n_runs(resid: &[f64]) -> usize {
    let mut runs = 1;
    for w in resid.windows(2) {
        let same = (w[0] < 0.0 && w[1] < 0.0) || (w[0] > 0.0 && w[1] > 0.0);
        if !same {
            runs += 1;
        }
    }
    runs
}

/// Result of [`find_best_regression`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Regression {
    /// Intercept at x = 0.
    pub b0: f64,
    /// Slope.
    pub b1: f64,
    /// Weighted average variance of the points.
    pub s2: f64,
    pub sb0: f64,
    pub sb1: f64,
    /// Number of oldest samples dropped to pass the runs test.
    pub new_start: usize,
    pub n_runs: usize,
    pub dof: usize,
}

/// Weighted linear regression over the newest samples, dropping the oldest
/// one at a time until the residuals pass the runs test (they look like
/// noise, not like an unmodelled trend). `x`, `y` hold `m` extra older
/// samples first (used only to extend the runs test) and then the `w.len()`
/// regression samples. Weights: larger = less reliable. `None` with fewer
/// than three samples.
pub fn find_best_regression(
    x: &[f64],
    y: &[f64],
    w: &[f64],
    m: usize,
    min_samples: usize,
) -> Option<Regression> {
    let n = w.len();
    assert!(x.len() == m + n && y.len() == m + n);
    if n < MIN_SAMPLES_FOR_REGRESS {
        return None;
    }
    let xs = &x[m..];
    let ys = &y[m..];
    let mut start = 0;
    let mut resid = Vec::with_capacity(n * REGRESS_RUNS_RATIO);
    loop {
        let (mut w_sum, mut u_sum) = (0.0, 0.0);
        for i in start..n {
            u_sum += xs[i] / w[i];
            w_sum += 1.0 / w[i];
        }
        let u = u_sum / w_sum;
        let (mut p, mut q, mut v) = (0.0, 0.0, 0.0);
        for i in start..n {
            let ui = xs[i] - u;
            p += ys[i] / w[i];
            q += ys[i] * ui / w[i];
            v += ui * ui / w[i];
        }
        let b = q / v;
        let a = p / w_sum - b * u;

        // Residuals, also for extra samples before `start` (back into the
        // m older ones), up to REGRESS_RUNS_RATIO times the regression size.
        let wanted = (n - start) * REGRESS_RUNS_RATIO;
        let resid_start = (m + n).saturating_sub(wanted); // index into x/y
        resid.clear();
        for i in resid_start..m + n {
            resid.push(y[i] - a - b * x[i]);
        }
        let mut nruns = n_runs(&resid);
        let critical = CRITICAL_RUNS.get(resid.len()).copied().unwrap_or(u8::MAX) as usize;
        if nruns > critical || n - start <= MIN_SAMPLES_FOR_REGRESS || n - start <= min_samples {
            let skip = m + start - resid_start;
            if skip != 0 {
                nruns = n_runs(&resid[skip..]);
            }
            let mut ss = 0.0;
            for i in start..n {
                let r = resid[m + i - resid_start];
                ss += r * r / w[i];
            }
            let npoints = n - start;
            ss /= (npoints - 2) as f64;
            let sb1 = (ss / v).sqrt();
            let aa = u * sb1;
            return Some(Regression {
                b0: a,
                b1: b,
                s2: ss * npoints as f64 / w_sum,
                sb0: (ss / w_sum + aa * aa).sqrt(),
                sb1,
                new_start: start,
                n_runs: nruns,
                dof: npoints - 2,
            });
        }
        start += 1;
    }
}

/// Slope of `y` against `x2` in the multiple regression y = b0 + b1 x1 + b2 x2
/// (chrony's `RGR_MultipleRegress`), used to estimate network asymmetry.
pub fn multiple_regress_b2(x1: &[f64], x2: &[f64], y: &[f64]) -> Option<f64> {
    let n = y.len();
    if n < 4 {
        return None;
    }
    let (mut sx1, mut sx2, mut sx1x1, mut sx1x2, mut sx2x2, mut sx1y, mut sx2y, mut sy) =
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for i in 0..n {
        sx1 += x1[i];
        sx2 += x2[i];
        sx1x1 += x1[i] * x1[i];
        sx1x2 += x1[i] * x2[i];
        sx2x2 += x2[i] * x2[i];
        sx1y += x1[i] * y[i];
        sx2y += x2[i] * y[i];
        sy += y[i];
    }
    let nf = n as f64;
    let u = nf * (sx1x2 * sx1y - sx1x1 * sx2y) + sx1 * sx1 * sx2y - sx1 * sx2 * sx1y
        + sy * (sx2 * sx1x1 - sx1 * sx1x2);
    let v1 = nf * (sx1x2 * sx1x2 - sx1x1 * sx2x2);
    let v2 = sx1 * sx1 * sx2x2 + sx2 * sx2 * sx1x1;
    let v3 = -2.0 * sx1 * sx2 * sx1x2;
    let v = v1 + v2 + v3;
    if v.abs() * 1.0e10 <= -v1 + v2 + v3.abs() {
        return None;
    }
    Some(u / v)
}

/// Median (by sorting a copy; n <= 64).
pub fn median(values: &[f64]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (*seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    }

    #[test]
    fn exact_line() {
        let x: Vec<f64> = (0..10).map(|i| -(9 - i) as f64 * 64.0).collect();
        let y: Vec<f64> = x.iter().map(|x| 0.002 + 20e-6 * x).collect();
        let w = vec![1.0; 10];
        let r = find_best_regression(&x, &y, &w, 0, 3).unwrap();
        assert!((r.b0 - 0.002).abs() < 1e-12);
        assert!((r.b1 - 20e-6).abs() < 1e-15);
    }

    #[test]
    fn noise_keeps_all_samples() {
        let mut seed = 3;
        let x: Vec<f64> = (0..32).map(|i| -(31 - i) as f64 * 64.0).collect();
        let y: Vec<f64> = x
            .iter()
            .map(|x| 1e-3 + 5e-6 * x + 100e-6 * lcg(&mut seed))
            .collect();
        let r = find_best_regression(&x, &y, &vec![1.0; 32], 0, 3).unwrap();
        assert_eq!(r.new_start, 0, "white noise passes the runs test");
        assert!((r.b1 - 5e-6).abs() < 1e-6);
    }

    #[test]
    fn runs_test_drops_samples_before_a_frequency_change() {
        // 20 ppm for 24 samples, then a temperature step to 26 ppm.
        let mut seed = 9;
        let n = 40;
        let mut y = Vec::new();
        let mut off = 0.0;
        for i in 0..n {
            off += if i < 24 { 20e-6 } else { 26e-6 } * 64.0;
            y.push(off + 20e-6 * lcg(&mut seed));
        }
        let x: Vec<f64> = (0..n).map(|i| -((n - 1 - i) as f64) * 64.0).collect();
        let r = find_best_regression(&x, &y, &vec![1.0; n], 0, 3).unwrap();
        assert!(
            r.new_start >= 14,
            "old trend dropped, start {}",
            r.new_start
        );
        assert!((r.b1 - 26e-6).abs() < 1e-6, "slope {}", r.b1);
    }

    #[test]
    fn runs_counting() {
        assert_eq!(n_runs(&[1.0, 2.0, -1.0, -2.0, 3.0]), 3);
        assert_eq!(n_runs(&[1.0]), 1);
        assert_eq!(n_runs(&[0.0, 0.0]), 2, "zero is its own run, as in chrony");
    }

    #[test]
    fn asymmetry_slope() {
        let x1: Vec<f64> = (0..10).map(|i| i as f64).collect();
        let x2: Vec<f64> = (0..10).map(|i| ((i * 7) % 5) as f64 * 0.01).collect();
        let y: Vec<f64> = (0..10).map(|i| 0.1 + 0.001 * x1[i] + 0.4 * x2[i]).collect();
        assert!((multiple_regress_b2(&x1, &x2, &y).unwrap() - 0.4).abs() < 1e-9);
    }
}
