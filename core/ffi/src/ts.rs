//! C ABI: the MPEG-TS muxer, feeding the SRT caller (or a callback in
//! tests and tools).

use std::sync::Arc;

use tallycam_core::sei::{self, Fields};
use tallycam_core::ts::{AudioCodec, Content, Muxer, VideoCodec, VideoFrame};

use crate::srt::send_code;

/// A muxer (`tallycam_core::ts`) whose TS packets go straight onto an SRT
/// caller's send queue, or to a C callback (tests and tools).
pub struct LcTsMux {
    muxer: Muxer,
    sender: Option<Arc<tallycam_srt::SrtSender>>,
    output: Option<(LcTsOutput, *mut std::ffi::c_void)>,
}

/// Receives the TS packets of one PES (or PAT + PMT) instead of the SRT
/// caller: `content` is LC_TS_CONTENT_*, `frame_capture_ns` as for
/// lc_srt_send. Returns what lc_srt_send would (payloads queued, or < 0).
pub type LcTsOutput = extern "C" fn(
    ctx: *mut std::ffi::c_void,
    data: *const u8,
    len: usize,
    content: u8,
    frame_capture_ns: i64,
) -> i32;

#[repr(C)]
pub struct LcTsVideoFrame {
    pub data: *const u8,
    pub len: usize,
    pub keyframe: bool,
    pub pts_ns: i64,
    pub send_capture_ns: i64,
    pub has_sei: bool,
    pub sei_capture_ns: u64,
    pub sei_flags: u8,
    pub sei_frame_counter: u32,
    pub sei_uncertainty_us: u32,
}

#[repr(C)]
pub struct LcTsStats {
    pub late_video_frames: u64,
    pub late_audio_frames: u64,
    pub started: bool,
    pub start_ns: i64,
    pub pcr_delay_ns: i64,
}

/// LC_TS_SKIPPED: the frame was not muxed (no format yet, or the stream has
/// not started).
const LC_TS_SKIPPED: i32 = 0;

/// Where a muxer's packets go, borrowed apart from the muxer itself.
struct Target<'a> {
    sender: &'a Option<Arc<tallycam_srt::SrtSender>>,
    output: &'a Option<(LcTsOutput, *mut std::ffi::c_void)>,
}

impl LcTsMux {
    fn target<'a>(
        sender: &'a Option<Arc<tallycam_srt::SrtSender>>,
        output: &'a Option<(LcTsOutput, *mut std::ffi::c_void)>,
    ) -> Target<'a> {
        Target { sender, output }
    }
}

impl Target<'_> {
    /// Sends one chunk; folds the result into `acc` (the first error wins,
    /// otherwise the payloads add up).
    fn emit(&self, ts: &[u8], content: Content, capture_ns: i64, acc: &mut i32) {
        let code = if let Some((output, ctx)) = *self.output {
            let c = match content {
                Content::Tables => 0,
                Content::Video => 1,
                Content::Audio => 2,
            };
            output(ctx, ts.as_ptr(), ts.len(), c, capture_ns)
        } else if let Some(sender) = self.sender {
            send_code(sender.send(ts, capture_ns, 0))
        } else {
            -1
        };
        if *acc >= 0 {
            *acc = if code < 0 { code } else { *acc + code };
        }
    }
}

/// A muxer sending on the SRT caller `srt_handle` (SrtCaller.hotHandle(); 0
/// for none, then set an output with lc_ts_set_output). `video_codec`
/// LC_TS_VIDEO_*, `audio_codec` LC_TS_AUDIO_*; `pcr_delay_ns` 0 picks the
/// default for the audio codec. NULL for an unknown codec.
///
/// # Safety
/// A non-zero `srt_handle` must come from a live `SrtCaller`.
#[no_mangle]
pub unsafe extern "C" fn lc_ts_new(
    srt_handle: u64,
    video_codec: u8,
    audio_codec: u8,
    pcr_delay_ns: i64,
) -> *mut LcTsMux {
    let video = match video_codec {
        0 => VideoCodec::H264,
        1 => VideoCodec::Hevc,
        _ => return std::ptr::null_mut(),
    };
    let audio = match audio_codec {
        0 => None,
        1 => Some(AudioCodec::Opus),
        2 => Some(AudioCodec::Aac),
        _ => return std::ptr::null_mut(),
    };
    let sender = (srt_handle != 0).then(|| {
        let ptr = srt_handle as *const tallycam_srt::SrtSender;
        // SAFETY: the handle is `Arc::as_ptr` of the caller's live sender;
        // take one more strong reference for the muxer.
        unsafe {
            Arc::increment_strong_count(ptr);
            Arc::from_raw(ptr)
        }
    });
    let delay = if pcr_delay_ns > 0 {
        pcr_delay_ns
    } else {
        AudioCodec::pcr_delay_ns(audio)
    };
    Box::into_raw(Box::new(LcTsMux {
        muxer: Muxer::new(video, audio, delay),
        sender,
        output: None,
    }))
}

/// # Safety
/// `mux` from `lc_ts_new`, not used afterwards.
#[no_mangle]
pub unsafe extern "C" fn lc_ts_free(mux: *mut LcTsMux) {
    if !mux.is_null() {
        // SAFETY: the caller passes a pointer from lc_ts_new, once.
        drop(unsafe { Box::from_raw(mux) });
    }
}

/// Sends to `output` instead of the SRT caller (NULL: back to the caller).
///
/// # Safety
/// `mux` from `lc_ts_new`; `ctx` stays valid while the output is set.
#[no_mangle]
pub unsafe extern "C" fn lc_ts_set_output(
    mux: *mut LcTsMux,
    output: Option<LcTsOutput>,
    ctx: *mut std::ffi::c_void,
) {
    if let Some(m) = unsafe { mux.as_mut() } {
        m.output = output.map(|o| (o, ctx));
    }
}

/// The video format: `parameter_sets` in Annex B (each set after a start
/// code; H.264 SPS and PPS, HEVC VPS, SPS and PPS), and the size of the
/// length prefix of the frames. Returns false (and drops video until a
/// valid format arrives) when a set is missing.
///
/// # Safety
/// `mux` from `lc_ts_new`; `parameter_sets` points to `len` bytes.
#[no_mangle]
pub unsafe extern "C" fn lc_ts_set_video_format(
    mux: *mut LcTsMux,
    parameter_sets: *const u8,
    len: usize,
    nal_length_size: u8,
) -> bool {
    let Some(m) = (unsafe { mux.as_mut() }) else {
        return false;
    };
    let sets: &[u8] = if parameter_sets.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the caller guarantees `len` readable bytes.
        unsafe { std::slice::from_raw_parts(parameter_sets, len) }
    };
    m.muxer
        .set_video_format(
            tallycam_core::ts::annexb::split(sets),
            nal_length_size.into(),
        )
        .is_ok()
}

/// The audio encoder's output format. False when the codec cannot carry it
/// (audio, and with it the stream start, then waits for a valid one).
///
/// # Safety
/// `mux` from `lc_ts_new`.
#[no_mangle]
pub unsafe extern "C" fn lc_ts_set_audio_format(
    mux: *mut LcTsMux,
    sample_rate: u32,
    channels: u8,
) -> bool {
    match unsafe { mux.as_mut() } {
        Some(m) => m.muxer.set_audio_format(sample_rate, channels).is_ok(),
        None => false,
    }
}

/// Muxes one video frame and queues it (with PAT and PMT before a
/// keyframe). Returns LC_TS_SKIPPED, the SRT payloads queued, or the first
/// LC_SRT_* error.
///
/// # Safety
/// `mux` from `lc_ts_new`, `frame` readable and its data `len` bytes; calls
/// for one muxer serialized.
#[no_mangle]
pub unsafe extern "C" fn lc_ts_video(
    mux: *mut LcTsMux,
    frame: *const LcTsVideoFrame,
    now_ns: i64,
) -> i32 {
    let (Some(m), Some(f)) = (unsafe { mux.as_mut() }, unsafe { frame.as_ref() }) else {
        return -4;
    };
    if f.data.is_null() || f.len == 0 {
        return -4;
    }
    // SAFETY: the caller guarantees `len` readable bytes.
    let data = unsafe { std::slice::from_raw_parts(f.data, f.len) };
    let sei = f.has_sei.then_some(Fields {
        capture_ns: f.sei_capture_ns,
        synced: f.sei_flags & sei::FLAG_SYNCED != 0,
        tai: f.sei_flags & sei::FLAG_TAI != 0,
        frame_counter: f.sei_frame_counter,
        uncertainty_us: f.sei_uncertainty_us,
    });
    let frame = VideoFrame {
        data,
        keyframe: f.keyframe,
        pts_ns: f.pts_ns,
        sei,
    };
    let mut acc = 0;
    let LcTsMux {
        muxer,
        sender,
        output,
    } = m;
    let target = LcTsMux::target(sender, output);
    let muxed = muxer.write_video(&frame, now_ns, &mut |ts, content| {
        let capture = if content == Content::Video {
            f.send_capture_ns
        } else {
            0
        };
        target.emit(ts, content, capture, &mut acc);
    });
    if muxed {
        acc
    } else {
        LC_TS_SKIPPED
    }
}

/// Muxes one audio frame (one Opus packet, or one raw AAC frame) with the
/// capture time of its first sample minus the encoder delay (`pts_ns`).
/// Returns as lc_ts_video.
///
/// # Safety
/// `mux` from `lc_ts_new`; `data` points to `len` bytes; calls for one
/// muxer serialized.
#[no_mangle]
pub unsafe extern "C" fn lc_ts_audio(
    mux: *mut LcTsMux,
    data: *const u8,
    len: usize,
    pts_ns: i64,
    now_ns: i64,
) -> i32 {
    let Some(m) = (unsafe { mux.as_mut() }) else {
        return -4;
    };
    if data.is_null() || len == 0 {
        return -4;
    }
    // SAFETY: the caller guarantees `len` readable bytes.
    let data = unsafe { std::slice::from_raw_parts(data, len) };
    let mut acc = 0;
    let LcTsMux {
        muxer,
        sender,
        output,
    } = m;
    let target = LcTsMux::target(sender, output);
    let muxed = muxer.write_audio(data, pts_ns, now_ns, &mut |ts, content| {
        target.emit(ts, content, 0, &mut acc);
    });
    if muxed {
        acc
    } else {
        LC_TS_SKIPPED
    }
}

/// True while the stream has not started because its first keyframe came
/// after its PCR deadline (the encoder warming up): the encoder should make
/// its next frame a keyframe.
///
/// # Safety
/// `mux` from `lc_ts_new`.
#[no_mangle]
pub unsafe extern "C" fn lc_ts_wants_keyframe(mux: *const LcTsMux) -> bool {
    unsafe { mux.as_ref() }.is_some_and(|m| m.muxer.wants_keyframe())
}

/// # Safety
/// `mux` from `lc_ts_new`, `out` writable.
#[no_mangle]
pub unsafe extern "C" fn lc_ts_stats(mux: *const LcTsMux, out: *mut LcTsStats) {
    let (Some(m), false) = (unsafe { mux.as_ref() }, out.is_null()) else {
        return;
    };
    let stats = LcTsStats {
        late_video_frames: m.muxer.late_video_frames(),
        late_audio_frames: m.muxer.late_audio_frames(),
        started: m.muxer.start_ns().is_some(),
        start_ns: m.muxer.start_ns().unwrap_or(0),
        pcr_delay_ns: m.muxer.clock().pcr_delay_ns,
    };
    // SAFETY: checked non-null; the caller guarantees it is writable.
    unsafe { out.write(stats) };
}

#[cfg(test)]
mod ts_ffi_tests {
    use super::*;
    use crate::srt::{srt_caller_options_from_url, SrtCaller, SrtEvent, SrtListener};

    extern "C" fn collect(
        ctx: *mut std::ffi::c_void,
        data: *const u8,
        len: usize,
        content: u8,
        capture_ns: i64,
    ) -> i32 {
        let out = unsafe { &mut *(ctx as *mut Vec<(u8, i64, Vec<u8>)>) };
        out.push((content, capture_ns, unsafe {
            std::slice::from_raw_parts(data, len).to_vec()
        }));
        len.div_ceil(1316) as i32
    }

    #[test]
    fn mux_through_the_c_api() {
        assert!(unsafe { lc_ts_new(0, 2, 0, 0) }.is_null());
        assert!(unsafe { lc_ts_new(0, 0, 3, 0) }.is_null());
        let m = unsafe { lc_ts_new(0, 0, 1, 0) };
        let mut out: Vec<(u8, i64, Vec<u8>)> = Vec::new();
        unsafe { lc_ts_set_output(m, Some(collect), &mut out as *mut _ as *mut _) };
        let sets = [0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE];
        assert!(!unsafe { lc_ts_set_video_format(m, sets.as_ptr(), 9, 4) });
        assert!(unsafe { lc_ts_set_video_format(m, sets.as_ptr(), sets.len(), 4) });
        assert!(unsafe { lc_ts_set_audio_format(m, 48_000, 1) });
        let slice = [0, 0, 0, 3, 0x65, 0x88, 0x84];
        let mut frame = LcTsVideoFrame {
            data: slice.as_ptr(),
            len: slice.len(),
            keyframe: true,
            pts_ns: 1_000_000_000,
            send_capture_ns: 999_000_000,
            has_sei: true,
            sei_capture_ns: 42,
            sei_flags: 0x03,
            sei_frame_counter: 7,
            sei_uncertainty_us: 5,
        };
        // Muxed after its PCR deadline (80 ms): the stream does not start
        // on it, and the muxer asks for another keyframe.
        assert_eq!(
            unsafe { lc_ts_video(m, &frame, 1_081_000_000) },
            LC_TS_SKIPPED
        );
        assert!(unsafe { lc_ts_wants_keyframe(m) });
        // Tables (1 payload) + video (1 payload).
        assert_eq!(unsafe { lc_ts_video(m, &frame, 1_030_000_000) }, 2);
        assert!(!unsafe { lc_ts_wants_keyframe(m) });
        assert_eq!(out[0].0, 0);
        assert_eq!((out[1].0, out[1].1), (1, 999_000_000));
        let es = &out[1].2;
        let sei_at = es.windows(5).position(|w| w == [0, 0, 0, 1, 0x06]).unwrap();
        let nal = tallycam_core::ts::annexb::split(&es[sei_at..])
            .next()
            .unwrap();
        let (_, f) = sei::parse_nal_unit(nal).unwrap();
        assert_eq!(
            (f.capture_ns, f.frame_counter, f.synced, f.tai),
            (42, 7, true, true)
        );
        let opus = [0x78, 1, 2];
        assert_eq!(
            unsafe { lc_ts_audio(m, opus.as_ptr(), 3, 999_000_000, 1_030_000_000) },
            LC_TS_SKIPPED,
            "before the first video frame"
        );
        assert_eq!(
            unsafe { lc_ts_audio(m, opus.as_ptr(), 3, 1_010_000_000, 1_030_000_000) },
            1
        );
        assert_eq!((out[2].0, out[2].1), (2, 0));
        frame.keyframe = false;
        frame.pts_ns = 1_033_000_000;
        assert_eq!(unsafe { lc_ts_video(m, &frame, 1_200_000_000) }, 1);
        let mut st = LcTsStats {
            late_video_frames: 0,
            late_audio_frames: 0,
            started: false,
            start_ns: 0,
            pcr_delay_ns: 0,
        };
        unsafe { lc_ts_stats(m, &mut st) };
        assert!(st.started);
        assert_eq!(st.start_ns, 1_000_000_000);
        assert_eq!(st.late_video_frames, 1);
        assert_eq!(st.pcr_delay_ns, 80_000_000);
        unsafe { lc_ts_free(m) };
    }

    #[test]
    fn mux_onto_an_srt_caller() {
        let mut options =
            srt_caller_options_from_url("srt://127.0.0.1:9?latency=0".into()).unwrap();
        options.connect_timeout_ms = 60_000;
        let caller = SrtCaller::start(options, Arc::new(NoListener)).unwrap();
        let m = unsafe { lc_ts_new(caller.hot_handle(), 1, 2, 0) };
        let sets = [
            0, 0, 0, 1, 0x40, 1, 0, 0, 0, 1, 0x42, 1, 0, 0, 0, 1, 0x44, 1,
        ];
        assert!(unsafe { lc_ts_set_video_format(m, sets.as_ptr(), sets.len(), 4) });
        let slice = [0, 0, 0, 3, 0x26, 0x01, 0x84];
        let frame = LcTsVideoFrame {
            data: slice.as_ptr(),
            len: slice.len(),
            keyframe: true,
            pts_ns: 1,
            send_capture_ns: 1,
            has_sei: false,
            sei_capture_ns: 0,
            sei_flags: 0,
            sei_frame_counter: 0,
            sei_uncertainty_us: 0,
        };
        // No audio format yet (AAC): not started.
        assert_eq!(unsafe { lc_ts_video(m, &frame, 2) }, LC_TS_SKIPPED);
        assert!(unsafe { lc_ts_set_audio_format(m, 48_000, 1) });
        // Muxed, but the caller is not connected.
        assert_eq!(unsafe { lc_ts_video(m, &frame, 2) }, -1);
        // The muxer keeps the sender alive past the caller.
        caller.close();
        drop(caller);
        assert_eq!(unsafe { lc_ts_video(m, &frame, 3) }, -1);
        unsafe { lc_ts_free(m) };
    }

    struct NoListener;

    impl SrtListener for NoListener {
        fn on_event(&self, _event: SrtEvent) {}
        fn on_frame_sent(&self, _: i64, _: i64, _: i64, _: bool) {}
    }
}
