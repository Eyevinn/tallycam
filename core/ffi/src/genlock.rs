//! C ABI: the soft genlock loop.

use tallycam_core::genlock::{Genlock, GenlockConfig, GenlockState};

/// Opaque genlock loop for C callers.
pub struct LcGenlock(Genlock);

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LcGenlockStatus {
    /// 0 free run, 1 acquiring, 2 locked.
    pub state: u8,
    pub phase_error_ns: f64,
    pub phase_error_max_ns: f64,
    pub frame_duration_ns: f64,
    pub correction_ppm: f64,
}

/// A loop for `fps_num / fps_den` that never asks for a frame shorter than
/// `min_frame_duration_ns`. Free it with `lc_genlock_free`.
#[no_mangle]
pub extern "C" fn lc_genlock_new(
    fps_num: u32,
    fps_den: u32,
    min_frame_duration_ns: f64,
) -> *mut LcGenlock {
    if fps_num == 0 || fps_den == 0 {
        return std::ptr::null_mut();
    }
    let cfg = GenlockConfig {
        fps_num,
        fps_den,
        min_frame_duration_ns,
        ..Default::default()
    };
    Box::into_raw(Box::new(LcGenlock(Genlock::new(cfg))))
}

/// # Safety
/// `g` must come from `lc_genlock_new` and not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn lc_genlock_free(g: *mut LcGenlock) {
    if !g.is_null() {
        // SAFETY: the caller passes a pointer from lc_genlock_new, once.
        drop(unsafe { Box::from_raw(g) });
    }
}

/// One captured frame (TAI ns, the clock's synced flag). Returns true and
/// writes the frame duration to ask for (ns) once per control window.
///
/// # Safety
/// `g` from `lc_genlock_new`; calls for one loop must be serialized.
/// `out_duration_ns` must be writable.
#[no_mangle]
pub unsafe extern "C" fn lc_genlock_on_frame(
    g: *mut LcGenlock,
    capture_tai_ns: i64,
    synced: bool,
    out_duration_ns: *mut f64,
) -> bool {
    if g.is_null() || out_duration_ns.is_null() {
        return false;
    }
    // SAFETY: the caller guarantees a live loop and serialized calls.
    let g = unsafe { &mut (*g).0 };
    match g.on_frame(capture_tai_ns, synced) {
        Some(d) => {
            // SAFETY: checked non-null; the caller guarantees it is writable.
            unsafe { out_duration_ns.write(d) };
            true
        }
        None => false,
    }
}

/// Moves the loop's target on the grid, ns after each grid point.
///
/// # Safety
/// `g` from `lc_genlock_new`; serialized with `lc_genlock_on_frame`.
#[no_mangle]
pub unsafe extern "C" fn lc_genlock_set_target_offset(g: *mut LcGenlock, offset_ns: i64) {
    if !g.is_null() {
        // SAFETY: the caller guarantees a live loop and serialized calls.
        unsafe { (*g).0.set_target_offset_ns(offset_ns) };
    }
}

/// # Safety
/// `g` from `lc_genlock_new`, `out` writable; not concurrent with
/// `lc_genlock_on_frame` on the same loop.
#[no_mangle]
pub unsafe extern "C" fn lc_genlock_status(g: *const LcGenlock, out: *mut LcGenlockStatus) {
    if g.is_null() || out.is_null() {
        return;
    }
    // SAFETY: the caller guarantees a live loop.
    let s = unsafe { &(*g).0 }.status();
    let status = LcGenlockStatus {
        state: match s.state {
            GenlockState::FreeRun => 0,
            GenlockState::Acquiring => 1,
            GenlockState::Locked => 2,
        },
        phase_error_ns: s.phase_error_ns,
        phase_error_max_ns: s.phase_error_max_ns,
        frame_duration_ns: s.frame_duration_ns,
        correction_ppm: s.correction_ppm,
    };
    // SAFETY: checked non-null; the caller guarantees it is writable.
    unsafe { out.write(status) };
}

#[cfg(test)]
mod genlock_ffi_tests {
    use super::*;

    #[test]
    fn genlock_round_trip_through_the_c_api() {
        assert!(lc_genlock_new(0, 1, 0.0).is_null());
        let g = lc_genlock_new(30, 1, 0.0);
        assert!(!g.is_null());
        let period = 1e9 / 30.0;
        let mut out = 0.0;
        let mut published = 0;
        // Frames 2 ms late on the grid: the loop asks for shorter frames.
        for i in 0..90i64 {
            let t = 1_800_000_000_000_000_000 + (i as f64 * period) as i64 + 2_000_000;
            if unsafe { lc_genlock_on_frame(g, t, true, &mut out) } {
                published += 1;
            }
        }
        assert_eq!(published, 3);
        assert!(out < period, "{out}");
        let mut st = LcGenlockStatus {
            state: 9,
            phase_error_ns: 0.0,
            phase_error_max_ns: 0.0,
            frame_duration_ns: 0.0,
            correction_ppm: 0.0,
        };
        unsafe { lc_genlock_status(g, &mut st) };
        assert_eq!(st.state, 1);
        assert!(st.correction_ppm < 0.0);
        unsafe { lc_genlock_free(g) };
    }
}
