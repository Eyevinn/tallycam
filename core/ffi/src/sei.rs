//! C ABI: the LCP capture timestamp SEI.

use tallycam_core::sei::{self, Codec, Fields};

/// # Safety
/// `out` must point to `capacity` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn lc_sei_write(
    capture_ns: u64,
    flags: u8,
    frame_counter: u32,
    uncertainty_us: u32,
    codec: u8,
    out: *mut u8,
    capacity: usize,
) -> usize {
    if out.is_null() || capacity < sei::MAX_NAL_LEN {
        return 0;
    }
    let codec = match codec {
        0 => Codec::H264,
        1 => Codec::Hevc,
        _ => return 0,
    };
    let fields = Fields {
        capture_ns,
        synced: flags & sei::FLAG_SYNCED != 0,
        tai: flags & sei::FLAG_TAI != 0,
        frame_counter,
        uncertainty_us,
    };
    // SAFETY: the caller guarantees `capacity` writable bytes at `out`.
    let buf = unsafe { std::slice::from_raw_parts_mut(out, capacity) };
    sei::write_nal_unit(&fields, codec, buf)
}

#[no_mangle]
pub extern "C" fn lc_sei_uncertainty_us(ns: i64) -> u32 {
    Fields::uncertainty_us_from_ns(if ns < 0 { None } else { Some(ns) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{lc_clock_map, lc_host_now_ns, LcClockMapping};

    #[test]
    fn hot_path_before_and_after_a_service() {
        let mut m = LcClockMapping {
            tai_ns: 0,
            utc_ns: 0,
            est_error_ns: 0,
            max_error_ns: 0,
            synced: true,
            disciplined: true,
        };
        let now = lc_host_now_ns();
        unsafe { lc_clock_map(now, &mut m) };
        assert!(!m.synced);
        assert_eq!(m.tai_ns - m.utc_ns, 37_000_000_000);
        let mut buf = [0u8; 76];
        let n = unsafe {
            lc_sei_write(
                m.tai_ns as u64,
                0x02,
                1,
                lc_sei_uncertainty_us(-1),
                0,
                buf.as_mut_ptr(),
                buf.len(),
            )
        };
        let (codec, f) = sei::parse_nal_unit(&buf[..n]).unwrap();
        assert_eq!(codec, Codec::H264);
        assert!(f.tai && !f.synced);
        assert_eq!(f.uncertainty_us, u32::MAX);
        assert_eq!(
            unsafe { lc_sei_write(0, 0, 0, 0, 0, buf.as_mut_ptr(), 10) },
            0
        );
    }
}
