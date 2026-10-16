//! Default and maximum video bitrate for a format and codec.
//!
//! The default of 1080p30 H.264 is 6 Mbit/s; it scales with the pixel rate
//! (sub-linearly: more pixels and frames compress better), HEVC needs about
//! 30% less. [`adaptive`] moves the bitrate below that with the network.

pub mod adaptive;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
}

pub const MIN_KBPS: u32 = 1000;
pub const ABSOLUTE_MAX_KBPS: u32 = 50_000;

/// Default bitrate, kbit/s, in steps of 500.
pub fn default_kbps(width: u32, height: u32, frame_rate: u32, codec: VideoCodec) -> u32 {
    let pixels = f64::from(width) * f64::from(height) / (1920.0 * 1080.0);
    let rate = f64::from(frame_rate.max(1)) / 30.0;
    let codec = if codec == VideoCodec::Hevc { 0.7 } else { 1.0 };
    let kbps = 6000.0 * pixels.powf(0.75) * rate.powf(0.6) * codec;
    ((kbps / 500.0).round() as u32 * 500).clamp(MIN_KBPS, ABSOLUTE_MAX_KBPS)
}

/// The top of the bitrate range: 2.5x the default, at least 20 Mbit/s (the
/// range of earlier builds), at most 50 Mbit/s.
pub fn max_kbps(width: u32, height: u32, frame_rate: u32, codec: VideoCodec) -> u32 {
    let kbps = default_kbps(width, height, frame_rate, codec) * 5 / 2;
    (kbps.div_ceil(1000) * 1000).clamp(20_000, ABSOLUTE_MAX_KBPS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use VideoCodec::{Hevc, H264};

    const P720: (u32, u32) = (1280, 720);
    const P1080: (u32, u32) = (1920, 1080);
    const P2160: (u32, u32) = (3840, 2160);

    fn kbps(r: (u32, u32), fps: u32, codec: VideoCodec) -> u32 {
        default_kbps(r.0, r.1, fps, codec)
    }

    fn max(r: (u32, u32), fps: u32, codec: VideoCodec) -> u32 {
        max_kbps(r.0, r.1, fps, codec)
    }

    #[test]
    fn scales_with_format_and_codec() {
        assert_eq!(kbps(P1080, 30, H264), 6000, "the default of earlier builds");
        assert_eq!(max(P1080, 30, H264), 20_000, "the range of earlier builds");
        assert!(kbps(P1080, 30, Hevc) < kbps(P1080, 30, H264));
        assert!(kbps(P1080, 30, H264) < kbps(P1080, 60, H264));
        assert!(kbps(P1080, 60, H264) < kbps(P1080, 240, H264));
        assert!(kbps(P720, 30, H264) < kbps(P1080, 30, H264));
        assert!(
            max(P2160, 60, Hevc) >= 40_000,
            "4K HEVC needs up to about 40 Mbit/s"
        );
        assert!(max(P2160, 60, H264) <= ABSOLUTE_MAX_KBPS);
        for r in [P720, P1080, P2160] {
            for fps in [24, 25, 30, 50, 60, 100, 120, 240] {
                if r == P2160 && fps > 60 {
                    continue;
                }
                for codec in [H264, Hevc] {
                    assert!(kbps(r, fps, codec) < max(r, fps, codec));
                    assert!(kbps(r, fps, codec) >= MIN_KBPS);
                }
            }
        }
    }

    #[test]
    fn spot_values() {
        assert_eq!(kbps(P720, 30, H264), 3500);
        assert_eq!(kbps(P1080, 60, H264), 9000);
        assert_eq!(kbps(P1080, 30, Hevc), 4000);
        assert_eq!(kbps(P2160, 30, H264), 17_000);
        assert_eq!(max(P2160, 30, H264), 43_000);
        assert_eq!(kbps(P2160, 60, H264), 25_500);
        assert_eq!(max(P2160, 60, H264), 50_000);
        assert_eq!(default_kbps(16, 16, 1, H264), MIN_KBPS);
    }
}
