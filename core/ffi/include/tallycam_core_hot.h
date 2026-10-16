// Per-frame C ABI of the TallyCam core (tallycam-core-ffi).
// No allocation, no locks: safe to call for every video frame and audio buffer.
#pragma once
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/// One mapped host instant.
typedef struct LcClockMapping {
    /// TAI ns since the PTP epoch (1970-01-01 00:00:00 TAI).
    int64_t tai_ns;
    /// UTC ns since the Unix epoch.
    int64_t utc_ns;
    /// Estimated error (1 sigma), ns.
    int64_t est_error_ns;
    /// Maximum error bound, ns.
    int64_t max_error_ns;
    /// Synchronized to enough agreeing sources.
    bool synced;
    /// False before a clock service published a mapping: the values then
    /// come from the device wall clock.
    bool disciplined;
} LcClockMapping;

/// Host clock now, ns (mach_absolute_time, the clock of capture timestamps).
int64_t lc_host_now_ns(void);

/// Host clock ticks (mach_absolute_time units, e.g. from
/// CMClockConvertHostTimeToSystemUnits) to ns.
int64_t lc_host_ticks_to_ns(uint64_t ticks);

/// Maps a host time in ns with the latest published clock mapping.
void lc_clock_map(int64_t host_ns, LcClockMapping *out);

/// Longest timestamp SEI NAL unit lc_sei_write produces.
#define LC_SEI_MAX_NAL_LEN 76
#define LC_SEI_FLAG_SYNCED 0x01
#define LC_SEI_FLAG_TAI 0x02
#define LC_SEI_CODEC_H264 0
#define LC_SEI_CODEC_HEVC 1

/// Writes the LCP timestamp SEI NAL unit (no start code or length prefix)
/// into `out` and returns its length, or 0 if `capacity` is below
/// LC_SEI_MAX_NAL_LEN or the codec is unknown.
size_t lc_sei_write(uint64_t capture_ns, uint8_t flags, uint32_t frame_counter, uint32_t uncertainty_us,
                    uint8_t codec, uint8_t *out, size_t capacity);

/// Microseconds for the SEI uncertainty field: rounded up, saturating;
/// negative means unknown (0xFFFFFFFF).
uint32_t lc_sei_uncertainty_us(int64_t ns);

/// Queues MPEG-TS bytes on an SRT caller: split into 1316-byte SRT payloads
/// (7 TS packets) and copied into a lock-free queue the SRT thread drains.
/// No allocation, no locks; the only syscall is a pipe write that wakes the
/// SRT thread when it sleeps. Calls for one handle must be serialized (one
/// producer).
///   handle            SrtCaller.hotHandle(); valid while that object lives
///   frame_capture_ns  host ns capture time when the bytes are one video
///                     frame (its first and last payloads are reported via
///                     SrtListener.onFrameSent), else 0
///   now_ns            host ns now (lc_host_now_ns), or 0 to read it here
/// Returns the payloads queued (>= 1), or one of the codes below; nothing is
/// queued then.
#define LC_SRT_NOT_CONNECTED (-1)
#define LC_SRT_QUEUE_FULL (-2) /* counted in SrtStats.droppedPackets */
#define LC_SRT_BUSY (-3)       /* another thread is in lc_srt_send */
#define LC_SRT_BAD_ARGS (-4)
int32_t lc_srt_send(uint64_t handle, const uint8_t *data, size_t len, int64_t frame_capture_ns, int64_t now_ns);

/// MPEG-TS muxer feeding an SRT caller: the app hands over encoded access
/// units and audio frames with their host times; the core builds the Annex B
/// access unit (AUD, parameter sets on keyframes, the timestamp SEI before
/// the first slice), muxes it (PAT/PMT before every keyframe, PTS = capture
/// time, PCR = now - PCR delay on the video PID) and queues the packets on
/// the SRT caller as lc_srt_send does (one call per PES: video frames are
/// timed through SrtListener.onFrameSent). Calls for one muxer must be
/// serialized, on the same thread or queue as any lc_srt_send to the same
/// caller. Only lc_ts_new and lc_ts_set_video_format allocate; the muxer's
/// buffers grow to the largest frame once and are reused.
typedef struct LcTsMux LcTsMux;

#define LC_TS_VIDEO_H264 0
#define LC_TS_VIDEO_HEVC 1
#define LC_TS_AUDIO_NONE 0
#define LC_TS_AUDIO_OPUS 1
#define LC_TS_AUDIO_AAC 2
#define LC_TS_CONTENT_TABLES 0
#define LC_TS_CONTENT_VIDEO 1
#define LC_TS_CONTENT_AUDIO 2
/// PCR delays (ns) lc_ts_new picks with pcr_delay_ns 0: AAC is the slow
/// stream with AAC audio; with Opus or no audio, video is.
#define LC_TS_PCR_DELAY_AAC_NS 150000000
#define LC_TS_PCR_DELAY_VIDEO_NS 80000000
/// lc_ts_video / lc_ts_audio: not muxed (no format yet, or the stream has
/// not started: it starts on a keyframe muxed before its PCR deadline, once
/// the audio format is known).
#define LC_TS_SKIPPED 0

typedef struct LcTsVideoFrame {
    /// The encoder's NAL units, each after a big-endian length of the size
    /// given to lc_ts_set_video_format (VideoToolbox output as is).
    const uint8_t *data;
    size_t len;
    bool keyframe;
    /// Host ns: the PTS (the capture time, calibrated).
    int64_t pts_ns;
    /// Host ns reported through SrtListener.onFrameSent for this frame.
    int64_t send_capture_ns;
    /// The LCP timestamp SEI (fields as for lc_sei_write).
    bool has_sei;
    uint64_t sei_capture_ns;
    uint8_t sei_flags;
    uint32_t sei_frame_counter;
    uint32_t sei_uncertainty_us;
} LcTsVideoFrame;

typedef struct LcTsStats {
    /// Samples muxed after their PCR deadline (PTS < PCR); should stay 0.
    uint64_t late_video_frames;
    uint64_t late_audio_frames;
    bool started;
    /// Host ns of the first muxed video frame, 0 before.
    int64_t start_ns;
    int64_t pcr_delay_ns;
} LcTsStats;

/// Receives the packets instead of the SRT caller (tests, tools). Returns
/// what lc_srt_send would.
typedef int32_t (*LcTsOutput)(void *ctx, const uint8_t *data, size_t len, uint8_t content,
                              int64_t frame_capture_ns);

/// srt_handle: SrtCaller.hotHandle() (the muxer keeps the caller's sender
/// alive), or 0 with lc_ts_set_output. pcr_delay_ns 0: the default for the
/// audio codec. NULL for an unknown codec.
LcTsMux *lc_ts_new(uint64_t srt_handle, uint8_t video_codec, uint8_t audio_codec, int64_t pcr_delay_ns);
void lc_ts_free(LcTsMux *mux);
void lc_ts_set_output(LcTsMux *mux, LcTsOutput output, void *ctx);
/// parameter_sets: Annex B, each set after a start code (H.264 SPS and PPS;
/// HEVC VPS, SPS and PPS). False if one is missing: video waits for a valid
/// format.
bool lc_ts_set_video_format(LcTsMux *mux, const uint8_t *parameter_sets, size_t len, uint8_t nal_length_size);
/// False if the codec cannot carry it (Opus: 48 kHz, 1 or 2 channels; AAC:
/// a rate with an ADTS index).
bool lc_ts_set_audio_format(LcTsMux *mux, uint32_t sample_rate, uint8_t channels);
/// Returns LC_TS_SKIPPED, the SRT payloads queued (>= 1), or the first
/// LC_SRT_* error (the frame was muxed, but not all of it queued). now_ns:
/// host ns now (lc_host_now_ns); unlike lc_srt_send, 0 is not "read it here".
int32_t lc_ts_video(LcTsMux *mux, const LcTsVideoFrame *frame, int64_t now_ns);
/// One Opus packet or one raw AAC frame (no ADTS header); pts_ns: capture
/// time of its first sample minus the encoder delay. Returns as lc_ts_video.
int32_t lc_ts_audio(LcTsMux *mux, const uint8_t *data, size_t len, int64_t pts_ns, int64_t now_ns);
/// True while the stream has not started because its first keyframe came
/// after its PCR deadline (the encoder warming up): make the next frame a
/// keyframe.
bool lc_ts_wants_keyframe(const LcTsMux *mux);
void lc_ts_stats(const LcTsMux *mux, LcTsStats *out);

/// Soft genlock: a PI loop that locks frame timing to the TAI frame grid.
/// Feed every captured frame; apply the returned frame duration (as the
/// device's min and max frame duration) when it changes. Calls for one loop
/// must be serialized. Only lc_genlock_new allocates.
typedef struct LcGenlock LcGenlock;

#define LC_GENLOCK_FREE_RUN 0
#define LC_GENLOCK_ACQUIRING 1
#define LC_GENLOCK_LOCKED 2

typedef struct LcGenlockStatus {
    uint8_t state;
    /// Mean phase error of the last window, ns (positive: frames late).
    double phase_error_ns;
    /// Largest |phase error| of one frame in the last window, ns.
    double phase_error_max_ns;
    /// The frame duration asked for, ns.
    double frame_duration_ns;
    /// Correction against the nominal period, ppm.
    double correction_ppm;
} LcGenlockStatus;

/// NULL if fps_num or fps_den is 0.
LcGenlock *lc_genlock_new(uint32_t fps_num, uint32_t fps_den, double min_frame_duration_ns);
void lc_genlock_free(LcGenlock *g);
/// Returns true and writes the frame duration to ask for, once per window.
bool lc_genlock_on_frame(LcGenlock *g, int64_t capture_tai_ns, bool synced, double *out_duration_ns);
void lc_genlock_status(const LcGenlock *g, LcGenlockStatus *out);
/// Moves the target on the grid: frames land offset_ns after each grid point
/// (per-model calibration). Keeps the learned rate.
void lc_genlock_set_target_offset(LcGenlock *g, int64_t offset_ns);

#ifdef __cplusplus
}
#endif

// ---------------------------------------------------------------------------
// Auto ISO (core/src/exposure.rs): the shutter stays fixed (rounded to a
// flicker-free value for the mains frequency) and the ISO follows the
// metered exposure. Feed a sample a few times a second (8 Hz); apply the
// returned ISO and exposure duration together. Calls for one loop must be
// serialized. Only lc_auto_iso_new allocates.
// ---------------------------------------------------------------------------
#ifdef __cplusplus
extern "C" {
#endif

typedef struct LcAutoIso LcAutoIso;

typedef struct LcAutoIsoInput {
    /// Monotonic time, ns.
    int64_t t_ns;
    /// Metered exposure against the camera's target, EV (exposureTargetOffset).
    double offset_ev;
    /// The device's current ISO and exposure duration.
    double iso;
    double exposure_ns;
    /// The active format's limits; max_exposure_ns from lc_longest_exposure_ns.
    double min_iso;
    double max_iso;
    double min_exposure_ns;
    double max_exposure_ns;
} LcAutoIsoInput;

#define LC_AUTO_ISO_LIMIT_NONE 0
#define LC_AUTO_ISO_LIMIT_TOO_DARK 1   /* at the highest ISO */
#define LC_AUTO_ISO_LIMIT_TOO_BRIGHT 2 /* at the lowest ISO */

typedef struct LcAutoIsoStatus {
    /// Filtered metered offset minus the bias, EV.
    double error_ev;
    /// Learned metered EV per EV of exposure.
    double gain;
    uint8_t limit;
    /// Waiting for the metering after a step.
    bool settling;
} LcAutoIsoStatus;

/// mains_hz 50 or 60 rounds the shutter flicker-free; 0 leaves it.
LcAutoIso *lc_auto_iso_new(uint32_t mains_hz);
void lc_auto_iso_free(LcAutoIso *a);
/// Starts over (auto ISO turned on); keeps the learned gain and the bias.
void lc_auto_iso_reset(LcAutoIso *a);
/// Target offset, EV (exposure compensation).
void lc_auto_iso_set_bias(LcAutoIso *a, double bias_ev);
void lc_auto_iso_set_mains(LcAutoIso *a, uint32_t mains_hz);
/// Returns true and writes the ISO and exposure duration to apply.
bool lc_auto_iso_on_sample(LcAutoIso *a, const LcAutoIsoInput *input, double *out_iso, double *out_exposure_ns);
void lc_auto_iso_status(const LcAutoIso *a, LcAutoIsoStatus *out);
/// The longest exposure for a frame period: the format's maximum, the frame,
/// and room for genlock's shorter frames while it runs.
double lc_longest_exposure_ns(double frame_period_ns, double format_max_ns, bool genlock);

#ifdef __cplusplus
}
#endif
