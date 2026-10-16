//! Android bindings of the TallyCam core, as one library
//! (`libtallycam_android.so`):
//!
//! - The UniFFI API of `tallycam-core-ffi` (clock service, SRT caller, device
//!   keys, camera defaults), linked in from its rlib; Kotlin bindings are
//!   generated from this library (`scripts/build-core-android.sh`).
//! - JNI wrappers of its per-frame C ABI (`se.eyevinn.tallycam.core.Hot`):
//!   the host clock, the clock mapping and the MPEG-TS muxer that queues onto
//!   the SRT caller. Frames come in direct `ByteBuffer`s, so nothing is copied
//!   on the way in and nothing is allocated per call.
//!
//! The wrappers call the C symbols of `tallycam-core-ffi` (declared below,
//! as in `core/ffi/include/tallycam_core_hot.h`) rather than its Rust
//! modules, so the Android side uses exactly the C ABI the iOS app uses.

use jni::objects::{JByteBuffer, JClass, JLongArray};
use jni::sys::{jboolean, jbyte, jint, jlong, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;

// Keeps the dependency linked, so its UniFFI and C symbols are in this
// library even though only the C symbols are referenced below.
pub use tallycam_core_ffi;

#[repr(C)]
struct LcClockMapping {
    tai_ns: i64,
    utc_ns: i64,
    est_error_ns: i64,
    max_error_ns: i64,
    synced: bool,
    disciplined: bool,
}

#[repr(C)]
struct LcTsVideoFrame {
    data: *const u8,
    len: usize,
    keyframe: bool,
    pts_ns: i64,
    send_capture_ns: i64,
    has_sei: bool,
    sei_capture_ns: u64,
    sei_flags: u8,
    sei_frame_counter: u32,
    sei_uncertainty_us: u32,
}

#[repr(C)]
struct LcTsStats {
    late_video_frames: u64,
    late_audio_frames: u64,
    started: bool,
    start_ns: i64,
    pcr_delay_ns: i64,
}

enum LcTsMux {}

extern "C" {
    fn lc_host_now_ns() -> i64;
    fn lc_clock_map(host_ns: i64, out: *mut LcClockMapping);
    fn lc_sei_uncertainty_us(ns: i64) -> u32;
    fn lc_ts_new(
        srt_handle: u64,
        video_codec: u8,
        audio_codec: u8,
        pcr_delay_ns: i64,
    ) -> *mut LcTsMux;
    fn lc_ts_free(mux: *mut LcTsMux);
    fn lc_ts_set_video_format(
        mux: *mut LcTsMux,
        parameter_sets: *const u8,
        len: usize,
        nal_length_size: u8,
    ) -> bool;
    fn lc_ts_set_audio_format(mux: *mut LcTsMux, sample_rate: u32, channels: u8) -> bool;
    fn lc_ts_video(mux: *mut LcTsMux, frame: *const LcTsVideoFrame, now_ns: i64) -> i32;
    fn lc_ts_audio(mux: *mut LcTsMux, data: *const u8, len: usize, pts_ns: i64, now_ns: i64)
        -> i32;
    fn lc_ts_stats(mux: *const LcTsMux, out: *mut LcTsStats);
    fn lc_ts_wants_keyframe(mux: *const LcTsMux) -> bool;
}

/// LC_SRT_BAD_ARGS: returned when a buffer is not direct or too short.
const BAD_ARGS: jint = -4;

fn as_bool(value: bool) -> jboolean {
    if value {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

/// The first `len` bytes of a direct buffer, or None.
fn direct<'a>(env: &JNIEnv, buffer: &JByteBuffer, len: jint) -> Option<&'a [u8]> {
    let ptr = env.get_direct_buffer_address(buffer).ok()?;
    let capacity = env.get_direct_buffer_capacity(buffer).ok()?;
    let len = usize::try_from(len).ok()?;
    if ptr.is_null() || len > capacity {
        return None;
    }
    // SAFETY: a direct buffer of at least `len` bytes, alive for the call.
    Some(unsafe { std::slice::from_raw_parts(ptr, len) })
}

#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_hostNowNs(
    _env: JNIEnv,
    _class: JClass,
) -> jlong {
    // SAFETY: no arguments.
    unsafe { lc_host_now_ns() }
}

/// Fills `out` (length 5): tai_ns, utc_ns, est_error_ns, max_error_ns, and
/// flags (bit 0 synced, bit 1 disciplined). One JNI call, no allocation.
#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_clockMap(
    env: JNIEnv,
    _class: JClass,
    host_ns: jlong,
    out: JLongArray,
) {
    let mut m = LcClockMapping {
        tai_ns: 0,
        utc_ns: 0,
        est_error_ns: 0,
        max_error_ns: 0,
        synced: false,
        disciplined: false,
    };
    // SAFETY: plain out-parameter.
    unsafe { lc_clock_map(host_ns, &mut m) };
    let flags = i64::from(m.synced) | (i64::from(m.disciplined) << 1);
    let values = [m.tai_ns, m.utc_ns, m.est_error_ns, m.max_error_ns, flags];
    let _ = env.set_long_array_region(&out, 0, &values);
}

#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_seiUncertaintyUs(
    _env: JNIEnv,
    _class: JClass,
    ns: jlong,
) -> jint {
    // SAFETY: plain value; the u32 is returned as its bit pattern.
    unsafe { lc_sei_uncertainty_us(ns) as jint }
}

/// Returns the muxer handle, or 0 for an unknown codec.
#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_tsNew(
    _env: JNIEnv,
    _class: JClass,
    srt_handle: jlong,
    video_codec: jbyte,
    audio_codec: jbyte,
    pcr_delay_ns: jlong,
) -> jlong {
    // SAFETY: the handle comes from SrtCaller.hotHandle(); the core checks it.
    unsafe {
        lc_ts_new(
            srt_handle as u64,
            video_codec as u8,
            audio_codec as u8,
            pcr_delay_ns,
        ) as jlong
    }
}

#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_tsFree(
    _env: JNIEnv,
    _class: JClass,
    mux: jlong,
) {
    if mux != 0 {
        // SAFETY: a handle from tsNew, freed once by the caller.
        unsafe { lc_ts_free(mux as *mut LcTsMux) }
    }
}

/// `parameter_sets`: Annex B (each set after a start code), as MediaCodec's
/// codec-config buffer is.
#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_tsSetVideoFormat(
    env: JNIEnv,
    _class: JClass,
    mux: jlong,
    parameter_sets: JByteBuffer,
    len: jint,
    nal_length_size: jbyte,
) -> jboolean {
    let Some(data) = direct(&env, &parameter_sets, len) else {
        return JNI_FALSE;
    };
    // SAFETY: a live muxer handle; data valid for the call.
    as_bool(unsafe {
        lc_ts_set_video_format(
            mux as *mut LcTsMux,
            data.as_ptr(),
            data.len(),
            nal_length_size as u8,
        )
    })
}

#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_tsSetAudioFormat(
    _env: JNIEnv,
    _class: JClass,
    mux: jlong,
    sample_rate: jint,
    channels: jint,
) -> jboolean {
    // SAFETY: a live muxer handle.
    as_bool(unsafe {
        lc_ts_set_audio_format(mux as *mut LcTsMux, sample_rate as u32, channels as u8)
    })
}

/// One encoded frame: NAL units each after a big-endian length of the size
/// given to tsSetVideoFormat. Returns as lc_ts_video.
#[allow(clippy::too_many_arguments)]
#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_tsVideo(
    env: JNIEnv,
    _class: JClass,
    mux: jlong,
    data: JByteBuffer,
    len: jint,
    keyframe: jboolean,
    pts_ns: jlong,
    send_capture_ns: jlong,
    sei_capture_ns: jlong,
    sei_flags: jint,
    sei_frame_counter: jint,
    sei_uncertainty_us: jint,
    now_ns: jlong,
) -> jint {
    let Some(data) = direct(&env, &data, len) else {
        return BAD_ARGS;
    };
    let frame = LcTsVideoFrame {
        data: data.as_ptr(),
        len: data.len(),
        keyframe: keyframe != JNI_FALSE,
        pts_ns,
        send_capture_ns,
        has_sei: sei_capture_ns > 0,
        sei_capture_ns: sei_capture_ns as u64,
        sei_flags: sei_flags as u8,
        sei_frame_counter: sei_frame_counter as u32,
        sei_uncertainty_us: sei_uncertainty_us as u32,
    };
    // SAFETY: a live muxer handle; the frame and its data valid for the call.
    unsafe { lc_ts_video(mux as *mut LcTsMux, &frame, now_ns) }
}

/// One Opus packet or raw AAC frame. Returns as lc_ts_audio.
#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_tsAudio(
    env: JNIEnv,
    _class: JClass,
    mux: jlong,
    data: JByteBuffer,
    len: jint,
    pts_ns: jlong,
    now_ns: jlong,
) -> jint {
    let Some(data) = direct(&env, &data, len) else {
        return BAD_ARGS;
    };
    // SAFETY: a live muxer handle; data valid for the call.
    unsafe {
        lc_ts_audio(
            mux as *mut LcTsMux,
            data.as_ptr(),
            data.len(),
            pts_ns,
            now_ns,
        )
    }
}

/// Fills `out` (length 5): late video frames, late audio frames, started
/// (0/1), start ns, PCR delay ns.
#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_tsStats(
    env: JNIEnv,
    _class: JClass,
    mux: jlong,
    out: JLongArray,
) {
    let mut s = LcTsStats {
        late_video_frames: 0,
        late_audio_frames: 0,
        started: false,
        start_ns: 0,
        pcr_delay_ns: 0,
    };
    // SAFETY: a live muxer handle; plain out-parameter.
    unsafe { lc_ts_stats(mux as *const LcTsMux, &mut s) };
    let values = [
        s.late_video_frames as i64,
        s.late_audio_frames as i64,
        i64::from(s.started),
        s.start_ns,
        s.pcr_delay_ns,
    ];
    let _ = env.set_long_array_region(&out, 0, &values);
}

/// True while the stream waits for a keyframe that makes its PCR deadline
/// (the encoder warming up): ask the encoder for a sync frame.
#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_tsWantsKeyframe(
    _env: JNIEnv,
    _class: JClass,
    mux: jlong,
) -> jboolean {
    // SAFETY: a live muxer handle (or 0, which the core treats as none).
    as_bool(unsafe { lc_ts_wants_keyframe(mux as *const LcTsMux) })
}

// MARK: Soft genlock (lc_genlock_*)

enum LcGenlock {}

#[repr(C)]
struct LcGenlockStatus {
    state: u8,
    phase_error_ns: f64,
    phase_error_max_ns: f64,
    frame_duration_ns: f64,
    correction_ppm: f64,
}

extern "C" {
    fn lc_genlock_new(fps_num: u32, fps_den: u32, min_frame_duration_ns: f64) -> *mut LcGenlock;
    fn lc_genlock_free(g: *mut LcGenlock);
    fn lc_genlock_on_frame(
        g: *mut LcGenlock,
        capture_tai_ns: i64,
        synced: bool,
        out_duration_ns: *mut f64,
    ) -> bool;
    fn lc_genlock_status(g: *const LcGenlock, out: *mut LcGenlockStatus);
    fn lc_genlock_set_target_offset(g: *mut LcGenlock, offset_ns: i64);
}

/// Returns the loop handle, or 0 for a zero frame rate.
#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_genlockNew(
    _env: JNIEnv,
    _class: JClass,
    fps_num: jint,
    fps_den: jint,
    min_frame_duration_ns: f64,
) -> jlong {
    // SAFETY: plain values.
    unsafe { lc_genlock_new(fps_num as u32, fps_den as u32, min_frame_duration_ns) as jlong }
}

#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_genlockFree(
    _env: JNIEnv,
    _class: JClass,
    g: jlong,
) {
    if g != 0 {
        // SAFETY: a handle from genlockNew, freed once by the caller.
        unsafe { lc_genlock_free(g as *mut LcGenlock) }
    }
}

/// Feeds one frame; returns the frame duration to apply (ns) once per
/// window, else 0.
#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_genlockOnFrame(
    _env: JNIEnv,
    _class: JClass,
    g: jlong,
    capture_tai_ns: jlong,
    synced: jboolean,
) -> f64 {
    let mut duration = 0.0;
    // SAFETY: a live handle; plain out-parameter.
    let apply = unsafe {
        lc_genlock_on_frame(
            g as *mut LcGenlock,
            capture_tai_ns,
            synced != JNI_FALSE,
            &mut duration,
        )
    };
    if apply {
        duration
    } else {
        0.0
    }
}

/// Fills `out` (length 5): state, phase error ns, max phase error ns, frame
/// duration ns, correction ppm.
#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_genlockStatus(
    env: JNIEnv,
    _class: JClass,
    g: jlong,
    out: jni::objects::JDoubleArray,
) {
    let mut s = LcGenlockStatus {
        state: 0,
        phase_error_ns: 0.0,
        phase_error_max_ns: 0.0,
        frame_duration_ns: 0.0,
        correction_ppm: 0.0,
    };
    // SAFETY: a live handle; plain out-parameter.
    unsafe { lc_genlock_status(g as *const LcGenlock, &mut s) };
    let values = [
        f64::from(s.state),
        s.phase_error_ns,
        s.phase_error_max_ns,
        s.frame_duration_ns,
        s.correction_ppm,
    ];
    let _ = env.set_double_array_region(&out, 0, &values);
}

#[no_mangle]
pub extern "system" fn Java_se_eyevinn_tallycam_core_Hot_genlockSetTargetOffset(
    _env: JNIEnv,
    _class: JClass,
    g: jlong,
    offset_ns: jlong,
) {
    // SAFETY: a live handle.
    unsafe { lc_genlock_set_target_offset(g as *mut LcGenlock, offset_ns) }
}
