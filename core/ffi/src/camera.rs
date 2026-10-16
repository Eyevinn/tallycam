//! Camera logic of the core for Swift: the auto ISO loop (C ABI, like the
//! genlock loop: no allocation per call), the mains frequency per region and
//! the bitrate defaults (UniFFI, not per frame).

use tallycam_core::bitrate;
use tallycam_core::exposure::{self, AutoIso, AutoIsoConfig, AutoIsoInput, AutoIsoLimit};

// ---------------------------------------------------------------------------
// UniFFI
// ---------------------------------------------------------------------------

/// The usual mains frequency (50 or 60) for an ISO 3166 region code.
#[uniffi::export]
pub fn mains_hz_for_region(region: String) -> u32 {
    exposure::mains_hz_for_region(&region)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BitrateCodec {
    H264,
    Hevc,
}

impl From<BitrateCodec> for bitrate::VideoCodec {
    fn from(c: BitrateCodec) -> Self {
        match c {
            BitrateCodec::H264 => Self::H264,
            BitrateCodec::Hevc => Self::Hevc,
        }
    }
}

/// Default video bitrate for a format and codec, kbit/s.
#[uniffi::export]
pub fn video_bitrate_default_kbps(
    width: u32,
    height: u32,
    frame_rate: u32,
    codec: BitrateCodec,
) -> u32 {
    bitrate::default_kbps(width, height, frame_rate, codec.into())
}

/// Top of the bitrate range for a format and codec, kbit/s.
#[uniffi::export]
pub fn video_bitrate_max_kbps(
    width: u32,
    height: u32,
    frame_rate: u32,
    codec: BitrateCodec,
) -> u32 {
    bitrate::max_kbps(width, height, frame_rate, codec.into())
}

/// Bottom of every bitrate range, kbit/s.
#[uniffi::export]
pub fn video_bitrate_min_kbps() -> u32 {
    bitrate::MIN_KBPS
}

// ---------------------------------------------------------------------------
// C ABI: auto ISO
// ---------------------------------------------------------------------------

/// Opaque auto ISO loop for C callers.
pub struct LcAutoIso(AutoIso);

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LcAutoIsoInput {
    pub t_ns: i64,
    pub offset_ev: f64,
    pub iso: f64,
    pub exposure_ns: f64,
    pub min_iso: f64,
    pub max_iso: f64,
    pub min_exposure_ns: f64,
    pub max_exposure_ns: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LcAutoIsoStatus {
    pub error_ev: f64,
    pub gain: f64,
    /// 0 none, 1 too dark (at the highest ISO), 2 too bright.
    pub limit: u8,
    pub settling: bool,
}

/// A loop rounding the shutter for `mains_hz` (50, 60; 0: no rounding).
#[no_mangle]
pub extern "C" fn lc_auto_iso_new(mains_hz: u32) -> *mut LcAutoIso {
    let cfg = AutoIsoConfig {
        mains_hz,
        ..Default::default()
    };
    Box::into_raw(Box::new(LcAutoIso(AutoIso::new(cfg))))
}

/// # Safety
/// `a` must come from `lc_auto_iso_new` and not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn lc_auto_iso_free(a: *mut LcAutoIso) {
    if !a.is_null() {
        // SAFETY: the caller passes a pointer from lc_auto_iso_new, once.
        drop(unsafe { Box::from_raw(a) });
    }
}

/// # Safety
/// `a` from `lc_auto_iso_new`; calls for one loop must be serialized.
#[no_mangle]
pub unsafe extern "C" fn lc_auto_iso_reset(a: *mut LcAutoIso) {
    if !a.is_null() {
        // SAFETY: the caller guarantees a live loop and serialized calls.
        unsafe { (*a).0.reset() };
    }
}

/// # Safety
/// As `lc_auto_iso_reset`.
#[no_mangle]
pub unsafe extern "C" fn lc_auto_iso_set_bias(a: *mut LcAutoIso, bias_ev: f64) {
    if !a.is_null() {
        // SAFETY: the caller guarantees a live loop and serialized calls.
        unsafe { (*a).0.set_bias_ev(bias_ev) };
    }
}

/// # Safety
/// As `lc_auto_iso_reset`.
#[no_mangle]
pub unsafe extern "C" fn lc_auto_iso_set_mains(a: *mut LcAutoIso, mains_hz: u32) {
    if !a.is_null() {
        // SAFETY: the caller guarantees a live loop and serialized calls.
        unsafe { (*a).0.set_mains_hz(mains_hz) };
    }
}

/// One sample. Returns true and writes the ISO and exposure duration (ns)
/// to apply when they change.
///
/// # Safety
/// `a` from `lc_auto_iso_new`, calls serialized; `input` readable, the
/// outputs writable.
#[no_mangle]
pub unsafe extern "C" fn lc_auto_iso_on_sample(
    a: *mut LcAutoIso,
    input: *const LcAutoIsoInput,
    out_iso: *mut f64,
    out_exposure_ns: *mut f64,
) -> bool {
    if a.is_null() || input.is_null() || out_iso.is_null() || out_exposure_ns.is_null() {
        return false;
    }
    // SAFETY: checked non-null; the caller guarantees validity.
    let (a, i) = unsafe { (&mut (*a).0, *input) };
    let input = AutoIsoInput {
        t_ns: i.t_ns,
        offset_ev: i.offset_ev,
        iso: i.iso,
        exposure_ns: i.exposure_ns,
        min_iso: i.min_iso,
        max_iso: i.max_iso,
        min_exposure_ns: i.min_exposure_ns,
        max_exposure_ns: i.max_exposure_ns,
    };
    match a.on_sample(&input) {
        Some(c) => {
            // SAFETY: checked non-null; the caller guarantees writability.
            unsafe {
                out_iso.write(c.iso);
                out_exposure_ns.write(c.exposure_ns);
            }
            true
        }
        None => false,
    }
}

/// # Safety
/// `a` from `lc_auto_iso_new`, `out` writable; serialized with the other calls.
#[no_mangle]
pub unsafe extern "C" fn lc_auto_iso_status(a: *const LcAutoIso, out: *mut LcAutoIsoStatus) {
    if a.is_null() || out.is_null() {
        return;
    }
    // SAFETY: the caller guarantees a live loop.
    let s = unsafe { &(*a).0 }.status();
    let status = LcAutoIsoStatus {
        error_ev: s.error_ev,
        gain: s.gain,
        limit: match s.limit {
            AutoIsoLimit::None => 0,
            AutoIsoLimit::TooDark => 1,
            AutoIsoLimit::TooBright => 2,
        },
        settling: s.settling,
    };
    // SAFETY: checked non-null; the caller guarantees writability.
    unsafe { out.write(status) };
}

/// The longest exposure for a frame period: the format's maximum, the
/// frame, and room for genlock's shorter frames while it runs.
#[no_mangle]
pub extern "C" fn lc_longest_exposure_ns(
    frame_period_ns: f64,
    format_max_ns: f64,
    genlock: bool,
) -> f64 {
    exposure::longest_exposure_ns(frame_period_ns, format_max_ns, genlock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_iso_round_trip_through_the_c_api() {
        let a = lc_auto_iso_new(50);
        let mut input = LcAutoIsoInput {
            t_ns: 0,
            offset_ev: 0.0,
            iso: 400.0,
            // 1/250 at 50 Hz: rounded to 1/100 with the ISO compensating.
            exposure_ns: 4_000_000.0,
            min_iso: 34.0,
            max_iso: 3264.0,
            min_exposure_ns: 14_000.0,
            max_exposure_ns: lc_longest_exposure_ns(1e9 / 30.0, 1e9, true),
        };
        let (mut iso, mut exposure) = (0.0, 0.0);
        assert!(unsafe { lc_auto_iso_on_sample(a, &input, &mut iso, &mut exposure) });
        assert_eq!(exposure, 10_000_000.0);
        assert!((iso - 160.0).abs() < 1e-6, "{iso}");
        let mut st = LcAutoIsoStatus {
            error_ev: 9.0,
            gain: 0.0,
            limit: 9,
            settling: false,
        };
        unsafe { lc_auto_iso_status(a, &mut st) };
        assert!(st.settling);
        assert_eq!(st.limit, 0);
        assert_eq!(st.gain, 3.0);
        // A bias of +1 metered EV: a third of an EV up (the initial gain is 3).
        input.iso = iso;
        input.exposure_ns = exposure;
        unsafe { lc_auto_iso_set_bias(a, 1.0) };
        for i in 1..20 {
            input.t_ns = i * 125_000_000;
            if unsafe { lc_auto_iso_on_sample(a, &input, &mut iso, &mut exposure) } {
                break;
            }
        }
        assert!((iso / 160.0 - (0.3f64).exp2()).abs() < 1e-6, "{iso}");
        unsafe { lc_auto_iso_free(a) };
    }
}
