//! Image statistics for exposure and colour (docs/SHADING.md, "Image
//! statistics"): what the phone's exposure warning, the hub's shader view
//! and, later, the auto-shader read.
//!
//! The platform samples a coarse grid of the picture that is encoded (after
//! the ISP), a few times a second at most: one luma byte and one Cb/Cr pair
//! per grid point (8-bit 4:2:0 gives both at every second pixel). This
//! module turns that grid into the `image_stats` numbers.
//!
//! Pure; one small allocation per call for the result.

use base64::Engine;

use crate::protocol::{ColourStats, ImageStats, LumaStats};

/// Luma bins in the reported histogram.
pub const HISTOGRAM_BINS: usize = 32;
/// Columns and levels of the reported waveform.
pub const WAVEFORM_COLUMNS: usize = 32;
pub const WAVEFORM_LEVELS: usize = 16;

/// Share of the range at each end counted as clipped or crushed.
const CLIP_MARGIN: f64 = 2.0 / 219.0;
/// Chroma distance (in 8-bit code values from neutral) within which a pixel
/// counts as near grey.
const NEUTRAL_CHROMA: i32 = 24;
/// Normalized luma range for near-grey pixels: too dark or too bright ones
/// carry no reliable colour.
const NEUTRAL_LUMA: (f64, f64) = (0.15, 0.9);

/// A sampled grid of an 8-bit Y'CbCr picture.
pub struct Grid<'a> {
    /// `width * height` luma samples, row by row.
    pub y: &'a [u8],
    /// `width * height` Cb, Cr pairs, row by row; empty without colour.
    pub cbcr: &'a [u8],
    pub width: usize,
    pub height: usize,
    /// Video range (Y 16..235, C 16..240) rather than full range.
    pub video_range: bool,
}

/// Luma normalized to 0..1 of the nominal range (may leave it slightly).
fn normalize(y: u8, video_range: bool) -> f64 {
    if video_range {
        (f64::from(y) - 16.0) / 219.0
    } else {
        f64::from(y) / 255.0
    }
}

/// The statistics of one grid, or None for an empty or inconsistent grid.
pub fn compute(grid: &Grid) -> Option<ImageStats> {
    let n = grid.width.checked_mul(grid.height)?;
    if n == 0 || grid.y.len() < n {
        return None;
    }
    let has_colour = grid.cbcr.len() >= 2 * n;

    // Fine histogram of normalized luma for the percentiles.
    let mut fine = [0u32; 256];
    let mut sum = 0.0;
    let mut clipped = 0u32;
    let mut crushed = 0u32;
    let mut coarse = [0u32; HISTOGRAM_BINS];
    let mut wave = [[0u32; WAVEFORM_LEVELS]; WAVEFORM_COLUMNS];
    let mut wave_totals = [0u32; WAVEFORM_COLUMNS];
    let (mut nr, mut ng, mut nb, mut neutral) = (0.0, 0.0, 0.0, 0u32);

    for row in 0..grid.height {
        for col in 0..grid.width {
            let i = row * grid.width + col;
            let v = normalize(grid.y[i], grid.video_range);
            let c = v.clamp(0.0, 1.0);
            sum += c;
            if v >= 1.0 - CLIP_MARGIN {
                clipped += 1;
            }
            if v <= CLIP_MARGIN {
                crushed += 1;
            }
            fine[((c * 255.0).round() as usize).min(255)] += 1;
            coarse[((c * HISTOGRAM_BINS as f64) as usize).min(HISTOGRAM_BINS - 1)] += 1;
            let wc = (col * WAVEFORM_COLUMNS / grid.width).min(WAVEFORM_COLUMNS - 1);
            let wl = ((c * WAVEFORM_LEVELS as f64) as usize).min(WAVEFORM_LEVELS - 1);
            wave[wc][wl] += 1;
            wave_totals[wc] += 1;

            if has_colour && (NEUTRAL_LUMA.0..=NEUTRAL_LUMA.1).contains(&c) {
                let cb = i32::from(grid.cbcr[2 * i]) - 128;
                let cr = i32::from(grid.cbcr[2 * i + 1]) - 128;
                if cb.abs() <= NEUTRAL_CHROMA && cr.abs() <= NEUTRAL_CHROMA {
                    let scale = if grid.video_range { 255.0 / 224.0 } else { 1.0 };
                    let (pb, pr) = (f64::from(cb) * scale / 255.0, f64::from(cr) * scale / 255.0);
                    // BT.709 Y'CbCr to R'G'B'.
                    nr += c + 1.5748 * pr;
                    ng += c - 0.1873 * pb - 0.4681 * pr;
                    nb += c + 1.8556 * pb;
                    neutral += 1;
                }
            }
        }
    }

    let total = n as f64;
    let percentile = |p: f64| {
        let target = (p * total).ceil().max(1.0) as u32;
        let mut seen = 0;
        for (bin, count) in fine.iter().enumerate() {
            seen += count;
            if seen >= target {
                return round3(bin as f64 / 255.0);
            }
        }
        1.0
    };
    let coarse_max = coarse.iter().copied().max().unwrap_or(0).max(1);
    let histogram: Vec<u8> = coarse
        .iter()
        .map(|&count| scale_byte(count, coarse_max))
        .collect();
    let mut waveform = Vec::with_capacity(WAVEFORM_COLUMNS * WAVEFORM_LEVELS);
    for (column, levels) in wave.iter().enumerate() {
        let column_total = wave_totals[column].max(1);
        waveform.extend(levels.iter().map(|&count| scale_byte(count, column_total)));
    }
    let b64 = base64::engine::general_purpose::STANDARD;

    let colour = has_colour.then(|| {
        let fraction = round3(f64::from(neutral) / total);
        if neutral == 0 || ng <= 0.0 {
            ColourStats {
                neutral_rg: None,
                neutral_bg: None,
                neutral_fraction: fraction,
            }
        } else {
            ColourStats {
                neutral_rg: Some(round3(nr / ng)),
                neutral_bg: Some(round3(nb / ng)),
                neutral_fraction: fraction,
            }
        }
    });

    Some(ImageStats {
        luma: LumaStats {
            mean: round3(sum / total),
            p01: percentile(0.01),
            p50: percentile(0.5),
            p99: percentile(0.99),
            clipped: round3(f64::from(clipped) / total),
            crushed: round3(f64::from(crushed) / total),
            histogram: b64.encode(&histogram),
            waveform: b64.encode(&waveform),
        },
        colour,
        camera: None,
    })
}

/// `count / of` as 0..=255.
fn scale_byte(count: u32, of: u32) -> u8 {
    ((f64::from(count) / f64::from(of)) * 255.0)
        .round()
        .min(255.0) as u8
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// The phone's exposure warning from the statistics: too dark when the
/// brightest 1% is still in the lower third, clipping when more than 2% of
/// the picture is at the top of the range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExposureWarning {
    TooDark,
    Clipping,
}

pub fn exposure_warning(stats: &LumaStats) -> Option<ExposureWarning> {
    if stats.clipped > 0.02 {
        Some(ExposureWarning::Clipping)
    } else if stats.p99 < 0.33 {
        Some(ExposureWarning::TooDark)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(y: &[u8], cbcr: &[u8], width: usize, height: usize) -> ImageStats {
        compute(&Grid {
            y,
            cbcr,
            width,
            height,
            video_range: true,
        })
        .unwrap()
    }

    #[test]
    fn mid_grey_is_neutral_and_centred() {
        let s = stats(&[126; 64], &[128; 128], 8, 8);
        assert!((s.luma.mean - 0.502).abs() < 0.002, "{}", s.luma.mean);
        assert_eq!(s.luma.clipped, 0.0);
        let c = s.colour.unwrap();
        assert_eq!(c.neutral_fraction, 1.0);
        assert_eq!(c.neutral_rg, Some(1.0));
        assert_eq!(c.neutral_bg, Some(1.0));
        assert_eq!(exposure_warning(&s.luma), None);
    }

    #[test]
    fn white_clips_and_black_is_too_dark() {
        let s = stats(&[235; 16], &[], 4, 4);
        assert_eq!(s.luma.clipped, 1.0);
        assert!(s.colour.is_none());
        assert_eq!(exposure_warning(&s.luma), Some(ExposureWarning::Clipping));

        let s = stats(&[17; 16], &[], 4, 4);
        assert_eq!(s.luma.crushed, 1.0);
        assert_eq!(exposure_warning(&s.luma), Some(ExposureWarning::TooDark));
    }

    #[test]
    fn warm_cast_shows_in_neutral_ratios() {
        // Grey with a little red (Cr up) and less blue (Cb down).
        let cbcr: Vec<u8> = (0..64).flat_map(|_| [120u8, 136u8]).collect();
        let s = stats(&[126; 64], &cbcr, 8, 8);
        let c = s.colour.unwrap();
        assert!(c.neutral_rg.unwrap() > 1.0);
        assert!(c.neutral_bg.unwrap() < 1.0);
    }

    #[test]
    fn histogram_and_waveform_have_fixed_sizes() {
        // Left half black, right half white.
        let y: Vec<u8> = (0..64 * 32)
            .map(|i| if i % 64 < 32 { 16 } else { 235 })
            .collect();
        let s = stats(&y, &[], 64, 32);
        let b64 = base64::engine::general_purpose::STANDARD;
        let hist = b64.decode(&s.luma.histogram).unwrap();
        let wave = b64.decode(&s.luma.waveform).unwrap();
        assert_eq!(hist.len(), HISTOGRAM_BINS);
        assert_eq!(wave.len(), WAVEFORM_COLUMNS * WAVEFORM_LEVELS);
        assert_eq!(hist[0], 255);
        assert_eq!(hist[HISTOGRAM_BINS - 1], 255);
        // First column all at level 0, last column all at the top level.
        assert_eq!(wave[0], 255);
        assert_eq!(
            wave[(WAVEFORM_COLUMNS - 1) * WAVEFORM_LEVELS + WAVEFORM_LEVELS - 1],
            255
        );
        assert!((s.luma.p50 - 0.0).abs() < 0.01 || (s.luma.p50 - 1.0).abs() < 0.01);
    }

    #[test]
    fn empty_or_short_grid_is_none() {
        let g = Grid {
            y: &[1, 2],
            cbcr: &[],
            width: 2,
            height: 2,
            video_range: true,
        };
        assert!(compute(&g).is_none());
    }
}
