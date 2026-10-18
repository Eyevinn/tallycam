//! Image statistics for Swift and Kotlin (docs/SHADING.md, "Image
//! statistics"). UniFFI; not per frame (statistics run a few times a second
//! at most).

use tallycam_core::image_stats::{self, ExposureWarning, Grid};

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ImageStatsResult {
    /// `{"luma": ..., "colour": ...}`, to merge into `image_stats`.
    pub json: String,
    pub mean: f64,
    pub clipped: f64,
    pub crushed: f64,
    /// `too_dark`, `clipping`, or None.
    pub warning: Option<String>,
}

/// Statistics of a sampled grid (`width * height` luma bytes and as many
/// Cb/Cr pairs, or no chroma). None for an inconsistent grid.
#[uniffi::export]
pub fn image_stats_compute(
    y: Vec<u8>,
    cbcr: Vec<u8>,
    width: u32,
    height: u32,
    video_range: bool,
) -> Option<ImageStatsResult> {
    let stats = image_stats::compute(&Grid {
        y: &y,
        cbcr: &cbcr,
        width: width as usize,
        height: height as usize,
        video_range,
    })?;
    let warning = image_stats::exposure_warning(&stats.luma).map(|w| match w {
        ExposureWarning::TooDark => "too_dark".to_string(),
        ExposureWarning::Clipping => "clipping".to_string(),
    });
    Some(ImageStatsResult {
        mean: stats.luma.mean,
        clipped: stats.luma.clipped,
        crushed: stats.luma.crushed,
        warning,
        json: serde_json::to_string(&stats).unwrap_or_default(),
    })
}
