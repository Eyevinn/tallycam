//! fake-camera: a simulated TallyCam camera for testing multi-camera
//! sync without phones.
//!
//! It generates test video with a running wall clock, a flash box and a frame
//! ruler, plus audio with a short beep on every reference second. Video is
//! encoded to H.264 with an LCP SEI capture timestamp (docs/PROTOCOL.md,
//! "Time sync and frame timestamps") in every access unit, audio to AAC, both
//! muxed to MPEG-TS and sent as an SRT caller.
//!
//! Timing model: the pipeline runs on the realtime system clock, with the base
//! time set to the last UTC midnight. Buffer running time is therefore the UTC
//! time of day, and `capture_ns = base_time + running_time` is the frame's
//! wall-clock capture time. Live test sources start their timestamps at the
//! running time they start in, so the video timestamps are moved forward
//! onto a grid: frames on whole second + `--phase-ms` + n / fps. The pipeline
//! goes to PLAYING just before a whole second, so the shift stays below one
//! frame. The audio is a sine gated to a 50 ms beep at the start of every
//! second of buffer time. The beep and the flash box mark the true reference
//! second. `--clock-offset-ms` only
//! shifts the stamped capture time, like a phone whose clock is wrong.

use anyhow::{anyhow, bail, Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use gstreamer_video::prelude::*;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tallycam_core::clock::driver::{
    default_host_clock, DisciplineDriver, DriverOptions, HostClock, StdResolver, SystemClockDriver,
};
use tallycam_core::clock::{ClockConfig, ClockSource, SnapshotCell};
use tallycam_core::protocol::ClockHealthLevel;
use tallycam_core::sei::{self, Codec, Fields};
use tallycam_core::ts::{self, Muxer};
use tallycam_srt::{CallerConfig, SenderOptions, SrtEvent, SrtObserver, SrtSender};
const SECOND_NS: u64 = 1_000_000_000;
const DAY_NS: u64 = 86_400 * SECOND_NS;
const AUDIO_RATE: u64 = 48_000;
const AUDIO_CHANNELS: u64 = 2;
/// Length of the beep at the start of every reference second.
const BEEP_NS: u64 = 50_000_000;
/// Every 5th second (UTC seconds divisible by 5) the beep is this long, so a
/// receiver can tell which second a beep belongs to, not only its offset
/// within a second.
const MARK_BEEP_NS: u64 = 150_000_000;

const USAGE: &str = "\
Usage: fake-camera [OPTIONS] [SRT_URL]

  SRT_URL                  srt://host:port[?streamid=...] (default srt://127.0.0.1:9100)
  --name NAME              name drawn in the picture (default CAM)
  --streamid ID            SRT stream id (alternative to ?streamid= in the URL)
  --fps N                  25, 30, 50 or 60 (default 30)
  --width W --height H     video size (default 1280x720)
  --latency-ms MS          SRT latency (default 200)
  --phase-ms MS            delay the frame grid by MS after the whole second (default 0)
  --clock-offset-ms MS     error added to the stamped capture time, may be negative (default 0)
  --unsynced               clear the synced flag and report unknown uncertainty
  --uncertainty-us US      reported sync uncertainty (default 500)
  --bitrate-kbps KBPS      video bitrate (default 2500)
  --encoder auto|x264|vt   H.264 encoder (default auto: x264enc, else vtenc_h264)
  --audio-codec aac|opus   audio codec (default aac); opus: opusenc, 10 ms frames, restricted low delay
  --duration-s S           stop after S seconds (default: run until killed)
  --system-clock           stamp the OS clock (UTC) instead of the core discipline (TAI)
  --local                  render picture and beep here (screen, speakers), do not send
  --full-flash             the whole picture flashes, not only the box (implied by --local)
  --audio-only             with --local: only the beep, no window
  --rust-srt               send with the core's Rust SRT caller instead of srtsink (libsrt)
  --core-ts                mux with the core's MPEG-TS muxer (as the app does) instead of
                           mpegtsmux; implies --rust-srt
";

#[derive(Debug, Clone)]
struct Args {
    url: String,
    streamid: Option<String>,
    name: String,
    fps: u32,
    width: u32,
    height: u32,
    latency_ms: u32,
    phase_ms: f64,
    clock_offset_ms: f64,
    unsynced: bool,
    uncertainty_us: u32,
    bitrate_kbps: u32,
    encoder: String,
    audio_codec: String,
    duration_s: Option<u64>,
    system_clock: bool,
    local: bool,
    full_flash: bool,
    audio_only: bool,
    /// Send with the core's Rust SRT caller (tallycam-srt) instead of srtsink.
    rust_srt: bool,
    /// Mux with the core's MPEG-TS muxer (tallycam_core::ts), as the app.
    core_ts: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            url: "srt://127.0.0.1:9100".into(),
            streamid: None,
            name: "CAM".into(),
            fps: 30,
            width: 1280,
            height: 720,
            latency_ms: 200,
            phase_ms: 0.0,
            clock_offset_ms: 0.0,
            unsynced: false,
            uncertainty_us: 500,
            bitrate_kbps: 2500,
            encoder: "auto".into(),
            audio_codec: "aac".into(),
            duration_s: None,
            system_clock: false,
            local: false,
            full_flash: false,
            audio_only: false,
            rust_srt: false,
            core_ts: false,
        }
    }
}

fn parse_args(raw: impl IntoIterator<Item = String>) -> Result<Args> {
    let mut args = Args::default();
    let mut it = raw.into_iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| anyhow!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--name" => args.name = value("--name")?,
            "--streamid" => args.streamid = Some(value("--streamid")?),
            "--fps" => args.fps = value("--fps")?.parse().context("--fps")?,
            "--width" => args.width = value("--width")?.parse().context("--width")?,
            "--height" => args.height = value("--height")?.parse().context("--height")?,
            "--latency-ms" => {
                args.latency_ms = value("--latency-ms")?.parse().context("--latency-ms")?
            }
            "--phase-ms" => args.phase_ms = value("--phase-ms")?.parse().context("--phase-ms")?,
            "--clock-offset-ms" => {
                args.clock_offset_ms = value("--clock-offset-ms")?
                    .parse()
                    .context("--clock-offset-ms")?
            }
            "--unsynced" => args.unsynced = true,
            "--uncertainty-us" => {
                args.uncertainty_us = value("--uncertainty-us")?
                    .parse()
                    .context("--uncertainty-us")?
            }
            "--bitrate-kbps" => {
                args.bitrate_kbps = value("--bitrate-kbps")?.parse().context("--bitrate-kbps")?
            }
            "--encoder" => args.encoder = value("--encoder")?,
            "--audio-codec" => args.audio_codec = value("--audio-codec")?,
            "--duration-s" => {
                args.duration_s = Some(value("--duration-s")?.parse().context("--duration-s")?)
            }
            "--system-clock" => args.system_clock = true,
            "--local" => {
                args.local = true;
                args.full_flash = true;
            }
            "--full-flash" => args.full_flash = true,
            "--audio-only" => args.audio_only = true,
            "--rust-srt" => args.rust_srt = true,
            "--core-ts" => {
                args.core_ts = true;
                args.rust_srt = true;
            }
            s if s.starts_with("srt://") => args.url = s.to_string(),
            other => bail!("unknown argument {other}\n\n{USAGE}"),
        }
    }
    if ![25, 30, 50, 60].contains(&args.fps) {
        bail!("--fps must be 25, 30, 50 or 60");
    }
    if args.width < 64 || args.height < 64 || args.width % 2 != 0 || args.height % 2 != 0 {
        bail!("--width and --height must be even and at least 64");
    }
    if !(0.0..1000.0).contains(&args.phase_ms) {
        bail!("--phase-ms must be in 0..1000");
    }
    if !["auto", "x264", "vt"].contains(&args.encoder.as_str()) {
        bail!("--encoder must be auto, x264 or vt");
    }
    if !matches!(args.audio_codec.as_str(), "aac" | "opus") {
        bail!("--audio-codec must be aac or opus");
    }
    Ok(args)
}

fn ms_to_ns(ms: f64) -> i64 {
    (ms * 1e6).round() as i64
}

// ---------------------------------------------------------------------------
// LCP SEI

/// The LCP timestamp SEI NAL unit from the core, with a 4-byte start code.
fn sei_nal(fields: &Fields) -> Vec<u8> {
    let mut nal = vec![0, 0, 0, 1];
    nal.extend_from_slice(&sei::nal_unit(fields, Codec::H264));
    nal
}

/// Maps system clock (UTC) times to the stamped timescale: TAI from the core
/// discipline, or UTC from the OS clock with `--system-clock`.
struct StampClock {
    host: Arc<dyn HostClock>,
    cell: Arc<SnapshotCell>,
    source: Arc<dyn ClockSource>,
    tai: bool,
    /// Stamped time minus system UTC, ns, refreshed by `refresh`.
    offset_ns: AtomicI64,
}

impl StampClock {
    fn start(system: bool) -> Arc<Self> {
        let host = default_host_clock();
        let cell = Arc::new(SnapshotCell::new());
        let source: Arc<dyn ClockSource> = if system {
            SystemClockDriver::start(host.clone(), cell.clone())
        } else {
            DisciplineDriver::start(
                ClockConfig::default(),
                host.clone(),
                Arc::new(StdResolver),
                DriverOptions::default(),
                cell.clone(),
            )
        };
        let clock = Arc::new(StampClock {
            host,
            cell,
            source,
            tai: !system,
            offset_ns: AtomicI64::new(0),
        });
        clock.refresh();
        clock
    }

    /// Re-reads the offset between the stamped timescale and system UTC.
    fn refresh(&self) {
        let h = self.host.now_ns();
        let wall = self.host.wall_utc_ns();
        if let Some(snap) = self.cell.load() {
            let stamped = if self.tai {
                snap.tai_ns(h)
            } else {
                snap.utc_ns(h)
            };
            self.offset_ns.store(stamped - wall, Ordering::Relaxed);
        }
    }

    fn stamp(&self, system_utc_ns: i64) -> i64 {
        system_utc_ns + self.offset_ns.load(Ordering::Relaxed)
    }

    /// Synchronized, no correction in flight (the core's `healthy`) and a
    /// maximum error below 10 ms: `healthy` alone allows hundreds of ms of
    /// max error right after start, more than the effects measured here.
    fn healthy(&self) -> bool {
        let s = self.source.status();
        s.health == ClockHealthLevel::Healthy && s.max_error_us <= 10_000
    }

    fn line(&self) -> String {
        let s = self.source.status();
        let tai_utc_ms = if self.tai { 37_000.0 } else { 0.0 };
        format!(
            "clock: {:?} synced {} est {:.2} ms; stamped - system clock {:+.1} ms ({})",
            s.health,
            s.synced,
            s.est_error_us as f64 / 1e3,
            self.offset_ns.load(Ordering::Relaxed) as f64 / 1e6 - tai_utc_ms,
            if self.tai {
                "TAI, shown minus 37 s"
            } else {
                "UTC"
            }
        )
    }
}

/// Inserts `nal` into an Annex B access unit right before the start code of
/// the first VCL NAL (slice), i.e. after any AUD/SPS/PPS/SEI. Returns `None`
/// when the access unit has no slice.
fn insert_before_first_slice(au: &[u8], nal: &[u8]) -> Option<Vec<u8>> {
    let mut i = 0;
    while i + 3 < au.len() {
        if au[i] == 0 && au[i + 1] == 0 && au[i + 2] == 1 {
            let nal_type = au[i + 3] & 0x1F;
            if (1..=5).contains(&nal_type) {
                let start = if i > 0 && au[i - 1] == 0 { i - 1 } else { i };
                let mut out = Vec::with_capacity(au.len() + nal.len());
                out.extend_from_slice(&au[..start]);
                out.extend_from_slice(nal);
                out.extend_from_slice(&au[start..]);
                return Some(out);
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Picture marks

/// Fills a rectangle of an I420 frame with one YUV colour.
fn fill_rect(
    frame: &mut gst_video::VideoFrameRef<&mut gst::BufferRef>,
    (x, y, w, h): (u32, u32, u32, u32),
    yuv: [u8; 3],
) {
    let (fw, fh) = (frame.width(), frame.height());
    let (x1, y1) = ((x + w).min(fw), (y + h).min(fh));
    if x >= x1 || y >= y1 {
        return;
    }
    let strides: Vec<usize> = frame.plane_stride().iter().map(|&s| s as usize).collect();
    for (plane, value) in yuv.iter().enumerate() {
        let shift = u32::from(plane > 0);
        let (px0, px1) = ((x >> shift) as usize, (x1 >> shift) as usize);
        let (py0, py1) = ((y >> shift) as usize, (y1 >> shift) as usize);
        let Ok(data) = frame.plane_data_mut(plane as u32) else {
            return;
        };
        for row in py0..py1 {
            let start = row * strides[plane];
            data[start + px0..start + px1].fill(*value);
        }
    }
}

/// Draws the flash box (lit on the frame that covers a whole reference second,
/// the same instant as the audio beep) and the frame ruler (one cell per frame
/// of the second, the cell of this frame lit).
fn draw_marks(
    buffer: &mut gst::BufferRef,
    info: &gst_video::VideoInfo,
    time_of_day_ns: u64,
    period_ns: u64,
    fps: u32,
    full: bool,
) {
    let Ok(mut frame) = gst_video::VideoFrameRef::from_buffer_ref_writable(buffer, info) else {
        return;
    };
    let in_second = time_of_day_ns % SECOND_NS;
    let flash = in_second == 0 || in_second + period_ns > SECOND_NS;
    let (w, h) = (info.width(), info.height());
    if full {
        let v = if flash {
            [235, 128, 128]
        } else {
            [16, 128, 128]
        };
        fill_rect(&mut frame, (0, 0, w, h), v);
        return;
    }

    let white = [235, 128, 128];
    let black = [16, 128, 128];
    let grey = [80, 128, 128];

    // Flash box, top right.
    let size = (h / 4) & !1;
    let rect = (w - size - w / 32, h / 16, size, size);
    fill_rect(&mut frame, rect, if flash { white } else { black });

    // Frame ruler along the bottom.
    let cell = (w / fps) & !1;
    let ruler_h = (h / 24).max(4) & !1;
    let y = (h - ruler_h - h / 48) & !1;
    let index = (in_second / period_ns) as u32;
    for i in 0..fps {
        let colour = if i == index { white } else { grey };
        let x = (i * cell + (w - cell * fps) / 2) & !1;
        fill_rect(
            &mut frame,
            (x + 2, y, cell.saturating_sub(4), ruler_h),
            colour,
        );
    }
}

/// Silences every sample of an interleaved S16 buffer that does not fall in
/// the first `BEEP_NS` of a second (`MARK_BEEP_NS` every 5th second). `pts_ns` is the time of day of sample 0.
fn gate_beep(buffer: &mut gst::BufferRef, pts_ns: u64) {
    let Ok(mut map) = buffer.map_writable() else {
        return;
    };
    let frame_bytes = 2 * AUDIO_CHANNELS as usize;
    for (i, frame) in map.chunks_exact_mut(frame_bytes).enumerate() {
        let t = pts_ns + i as u64 * SECOND_NS / AUDIO_RATE;
        let length = if (t / SECOND_NS).is_multiple_of(5) {
            MARK_BEEP_NS
        } else {
            BEEP_NS
        };
        if t % SECOND_NS >= length {
            frame.fill(0);
        }
    }
}

// ---------------------------------------------------------------------------
// Pipeline

fn make(factory: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .build()
        .with_context(|| format!("GStreamer element {factory} is missing"))
}

fn has(factory: &str) -> bool {
    gst::ElementFactory::find(factory).is_some()
}

fn video_encoder(args: &Args) -> Result<gst::Element> {
    let use_x264 = match args.encoder.as_str() {
        "x264" => true,
        "vt" => false,
        _ => has("x264enc"),
    };
    if use_x264 {
        let enc = make("x264enc")?;
        enc.set_property_from_str("tune", "zerolatency");
        enc.set_property_from_str("speed-preset", "ultrafast");
        enc.set_property("key-int-max", 30u32);
        enc.set_property("bitrate", args.bitrate_kbps);
        Ok(enc)
    } else {
        let enc = make("vtenc_h264")?;
        enc.set_property("realtime", true);
        enc.set_property("allow-frame-reordering", false);
        enc.set_property("max-keyframe-interval", 30i32);
        enc.set_property("bitrate", args.bitrate_kbps);
        Ok(enc)
    }
}

/// The audio encoder, its parser and its encoder delay (priming) in
/// samples. Decoders do not skip the priming in MPEG-TS, so the PTS must be
/// moved back by it for the audio to line up with the video (the app does
/// the same for Apple's encoders: 2112 samples for AAC, 312 for Opus).
fn audio_encoder(codec: &str) -> Result<(gst::Element, gst::Element, u64)> {
    if codec == "opus" {
        let enc = make("opusenc")?;
        enc.set_property_from_str("frame-size", "10");
        enc.set_property_from_str("audio-type", "restricted-lowdelay");
        enc.set_property("bitrate", 80_000i32);
        // Restricted low delay: 2.5 ms (120 samples) of pre-skip. mpegtsmux
        // signals it only as a start trim on the first packet, which
        // receivers apply once, so the PTS still need moving back.
        return Ok((enc, make("opusparse")?, OPUS_PRE_SKIP));
    }
    // avenc_aac: FFmpeg's initial_padding. Measured with sync-probe --av:
    // without the correction the beep is 21.3 ms (1024 samples) late.
    let (enc, delay) = [("avenc_aac", 1024), ("fdkaacenc", 2048), ("voaacenc", 0)]
        .into_iter()
        .find(|(f, _)| has(f))
        .map(|(f, delay)| make(f).map(|e| (e, delay)))
        .unwrap_or_else(|| bail!("no AAC encoder found (avenc_aac, fdkaacenc or voaacenc)"))?;
    Ok((enc, make("aacparse")?, delay))
}

/// opusenc's pre-skip with `audio-type=restricted-lowdelay` at 48 kHz.
const OPUS_PRE_SKIP: u64 = 120;

/// First grid point at or after `t`, where the grid is `phase_ns` plus
/// `n * 1 s / per_second` within every second.
fn next_grid_point(t: u64, phase_ns: u64, per_second: u64) -> u64 {
    let Some(rel) = t.checked_sub(phase_ns) else {
        return phase_ns;
    };
    let second = rel - rel % SECOND_NS;
    let in_second = u128::from(rel % SECOND_NS);
    let per = u128::from(per_second);
    let sec = u128::from(SECOND_NS);
    let n = (in_second * per).div_ceil(sec);
    phase_ns + second + (n * sec / per) as u64
}

/// Moves every buffer on `pad` by one constant shift, chosen on the first
/// buffer so that it lands on the next grid point (see `next_grid_point`).
fn align_to_grid(pad: &gst::Pad, phase_ns: u64, per_second: u64) {
    let shift = AtomicU64::new(u64::MAX);
    pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
        let Some(buffer) = info.buffer_mut() else {
            return gst::PadProbeReturn::Ok;
        };
        let Some(pts) = buffer.pts() else {
            return gst::PadProbeReturn::Ok;
        };
        let mut s = shift.load(Ordering::Relaxed);
        if s == u64::MAX {
            s = next_grid_point(pts.nseconds(), phase_ns, per_second) - pts.nseconds();
            shift.store(s, Ordering::Relaxed);
        }
        buffer
            .make_mut()
            .set_pts(gst::ClockTime::from_nseconds(pts.nseconds() + s));
        gst::PadProbeReturn::Ok
    });
}

fn build(
    args: &Args,
    base_ns: u64,
    stamp: Arc<StampClock>,
    rust_srt: Option<Arc<SrtSender>>,
) -> Result<gst::Pipeline> {
    let pipeline = gst::Pipeline::with_name("fake-camera");
    let period_ns = SECOND_NS / u64::from(args.fps);

    // Video: source, marks, overlays, encoder, parser.
    let vsrc = make("videotestsrc")?;
    vsrc.set_property("is-live", true);
    let video_info =
        gst_video::VideoInfo::builder(gst_video::VideoFormat::I420, args.width, args.height)
            .fps(gst::Fraction::new(args.fps as i32, 1))
            .build()?;
    let vcaps = make("capsfilter")?;
    vcaps.set_property("caps", video_info.to_caps()?);
    let info_text = make("textoverlay")?;
    info_text.set_property(
        "text",
        format!(
            "{} fps  phase {} ms  clock offset {} ms  {}",
            args.fps,
            args.phase_ms,
            args.clock_offset_ms,
            if args.unsynced { "unsynced" } else { "synced" }
        ),
    );
    info_text.set_property("font-desc", "Sans 16");
    info_text.set_property_from_str("halignment", "left");
    info_text.set_property_from_str("valignment", "top");
    info_text.set_property("shaded-background", true);
    let clock_text = make("timeoverlay")?;
    clock_text.set_property("text", format!("{}  UTC", args.name));
    clock_text.set_property("font-desc", "Sans Bold 40");
    clock_text.set_property_from_str("halignment", "center");
    clock_text.set_property_from_str("valignment", "center");
    clock_text.set_property("shaded-background", true);
    // Audio: a 1 kHz sine, gated to a beep at the start of every reference
    // second (audiotestsrc's own ticks drift by one buffer after the first).
    let asrc = make("audiotestsrc")?;
    asrc.set_property("is-live", true);
    asrc.set_property("freq", 1000.0f64);
    asrc.set_property("volume", 0.5f64);
    asrc.set_property("samplesperbuffer", 480i32); // 10 ms
    let acaps = make("capsfilter")?;
    acaps.set_property(
        "caps",
        gst::Caps::builder("audio/x-raw")
            .field("format", "S16LE")
            .field("layout", "interleaved")
            .field("rate", AUDIO_RATE as i32)
            .field("channels", AUDIO_CHANNELS as i32)
            .build(),
    );
    let aconv = make("audioconvert")?;
    pipeline.add_many([
        &vsrc,
        &vcaps,
        &info_text,
        &clock_text,
        &asrc,
        &acaps,
        &aconv,
    ])?;
    gst::Element::link_many([&vsrc, &vcaps, &info_text, &clock_text])?;
    gst::Element::link_many([&asrc, &acaps, &aconv])?;

    let mut h264_caps = None;
    if args.local {
        // Render here: the reference a phone films for A/V sync tests.
        let vconv = make("videoconvert")?;
        let vsink = if args.audio_only {
            // The pictures still run (they pace nothing), but are not shown.
            make("fakesink")?
        } else {
            make("autovideosink")?
        };
        vsink.set_property("sync", true);
        let ares = make("audioresample")?;
        let asink = make("autoaudiosink")?;
        pipeline.add_many([&vconv, &vsink, &ares, &asink])?;
        gst::Element::link_many([&clock_text, &vconv, &vsink])?;
        gst::Element::link_many([&aconv, &ares, &asink])?;
    } else {
        let venc = video_encoder(args)?;
        let vparse = make("h264parse")?;
        vparse.set_property("config-interval", -1i32);
        let h264_caps_filter = make("capsfilter")?;
        h264_caps_filter.set_property(
            "caps",
            gst::Caps::builder("video/x-h264")
                // The core muxer takes length-prefixed NAL units, as from
                // VideoToolbox in the app.
                .field(
                    "stream-format",
                    if args.core_ts { "avc" } else { "byte-stream" },
                )
                .field("alignment", "au")
                .build(),
        );
        let vqueue = make("queue")?;
        let (aenc, aparse, priming) = audio_encoder(&args.audio_codec)?;
        let priming_ns = priming * SECOND_NS / AUDIO_RATE;
        aenc.static_pad("src")
            .unwrap()
            .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                if let Some(buffer) = info.buffer_mut() {
                    if let Some(pts) = buffer.pts() {
                        let pts = pts.nseconds().saturating_sub(priming_ns);
                        buffer
                            .make_mut()
                            .set_pts(gst::ClockTime::from_nseconds(pts));
                    }
                }
                gst::PadProbeReturn::Ok
            });
        let aqueue = make("queue")?;

        if args.core_ts {
            let sender = rust_srt.ok_or_else(|| anyhow!("--core-ts needs the Rust SRT caller"))?;
            // AAC without ADTS headers, as Apple's encoder emits it.
            let araw = make("capsfilter")?;
            if args.audio_codec == "aac" {
                araw.set_property(
                    "caps",
                    gst::Caps::builder("audio/mpeg")
                        .field("stream-format", "raw")
                        .build(),
                );
            }
            let (vsink, asink) = core_ts_sinks(args, base_ns, stamp, sender);
            pipeline.add_many([
                &venc,
                &vparse,
                &h264_caps_filter,
                &vqueue,
                vsink.upcast_ref(),
                &aenc,
                &aparse,
                &araw,
                &aqueue,
                asink.upcast_ref(),
            ])?;
            gst::Element::link_many([
                &clock_text,
                &venc,
                &vparse,
                &h264_caps_filter,
                &vqueue,
                vsink.upcast_ref(),
            ])?;
            gst::Element::link_many([&aconv, &aenc, &aparse, &araw, &aqueue, asink.upcast_ref()])?;
            // The muxer writes the SEI itself: no probe below.
            let phase_ns = ms_to_ns(args.phase_ms) as u64;
            align_to_grid(
                &vsrc.static_pad("src").unwrap(),
                phase_ns,
                u64::from(args.fps),
            );
            add_marks(args, &acaps, &vcaps, video_info, period_ns);
            return Ok(pipeline);
        }

        // Mux and send.
        let mux = make("mpegtsmux")?;
        mux.set_property("alignment", 7i32);
        let sink = make("srtsink")?;
        let sink = if let Some(sender) = rust_srt {
            // Same timing as srtsink: a base sink that syncs on the clock.
            let sync: bool = sink.property("sync");
            let appsink = gst_app::AppSink::builder().sync(sync).build();
            appsink.set_callbacks(
                gst_app::AppSinkCallbacks::builder()
                    .new_sample(move |sink| {
                        let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        if let Some(map) = sample.buffer().and_then(|b| b.map_readable().ok()) {
                            let _ = sender.send(&map, 0, 0);
                        }
                        Ok(gst::FlowSuccess::Ok)
                    })
                    .build(),
            );
            appsink.upcast()
        } else {
            sink.set_property("uri", &args.url);
            sink.set_property("latency", args.latency_ms as i32);
            if let Some(id) = &args.streamid {
                sink.set_property("streamid", id);
            }
            sink
        };
        pipeline.add_many([
            &venc,
            &vparse,
            &h264_caps_filter,
            &vqueue,
            &aenc,
            &aparse,
            &aqueue,
            &mux,
            &sink,
        ])?;
        gst::Element::link_many([
            &clock_text,
            &venc,
            &vparse,
            &h264_caps_filter,
            &vqueue,
            &mux,
        ])?;
        gst::Element::link_many([&aconv, &aenc, &aparse, &aqueue, &mux])?;
        mux.link(&sink)?;
        h264_caps = Some(h264_caps_filter);
    }

    // Frame grid: whole second + phase + n / fps.
    let phase_ns = ms_to_ns(args.phase_ms) as u64;
    align_to_grid(
        &vsrc.static_pad("src").unwrap(),
        phase_ns,
        u64::from(args.fps),
    );

    add_marks(args, &acaps, &vcaps, video_info, period_ns);

    // LCP SEI in every access unit, between parser and mux.
    let Some(h264_caps) = h264_caps else {
        return Ok(pipeline);
    };
    let clock_offset_ns = ms_to_ns(args.clock_offset_ms);
    let synced = !args.unsynced;
    let uncertainty_us = if synced {
        args.uncertainty_us
    } else {
        sei::UNCERTAINTY_UNKNOWN
    };
    let tai = stamp.tai;
    let frame_counter = AtomicU32::new(0);
    h264_caps
        .static_pad("src")
        .unwrap()
        .add_probe(gst::PadProbeType::BUFFER, move |pad, info| {
            let Some(buffer) = info.buffer() else {
                return gst::PadProbeReturn::Ok;
            };
            let Some(pts) = buffer.pts() else {
                return gst::PadProbeReturn::Ok;
            };
            let running_time = pad
                .sticky_event::<gst::event::Segment>(0)
                .and_then(|ev| {
                    ev.segment()
                        .downcast_ref::<gst::ClockTime>()
                        .and_then(|s| s.to_running_time(pts))
                })
                .unwrap_or(pts);
            // System UTC of the capture, on the stamped timescale.
            let system_ns = base_ns as i64 + running_time.nseconds() as i64;
            let capture_ns = (stamp.stamp(system_ns) + clock_offset_ns).max(0) as u64;
            let fields = Fields {
                capture_ns,
                synced,
                tai,
                frame_counter: frame_counter.fetch_add(1, Ordering::Relaxed),
                uncertainty_us,
            };
            let nal = sei_nal(&fields);
            let Ok(map) = buffer.map_readable() else {
                return gst::PadProbeReturn::Ok;
            };
            let Some(data) = insert_before_first_slice(&map, &nal) else {
                return gst::PadProbeReturn::Ok;
            };
            drop(map);
            let mut out = gst::Buffer::from_mut_slice(data);
            {
                let out_ref = out.get_mut().unwrap();
                let flags = gst::BufferCopyFlags::FLAGS
                    | gst::BufferCopyFlags::TIMESTAMPS
                    | gst::BufferCopyFlags::META;
                let _ = buffer.copy_into(out_ref, flags, ..);
            }
            info.data = Some(gst::PadProbeData::Buffer(out));
            gst::PadProbeReturn::Ok
        });

    Ok(pipeline)
}

/// The beep gate on the raw audio and the flash box and frame ruler on the
/// raw frames (buffer running time is the UTC time of day).
fn add_marks(
    args: &Args,
    acaps: &gst::Element,
    vcaps: &gst::Element,
    video_info: gst_video::VideoInfo,
    period_ns: u64,
) {
    // Beep: keep the sine only in the first BEEP_NS of every second.
    acaps
        .static_pad("src")
        .unwrap()
        .add_probe(gst::PadProbeType::BUFFER, |_, info| {
            if let Some(buffer) = info.buffer_mut() {
                if let Some(pts) = buffer.pts() {
                    gate_beep(buffer.make_mut(), pts.nseconds());
                }
            }
            gst::PadProbeReturn::Ok
        });

    // Flash box and frame ruler on the raw frames. The buffer running time is
    // the UTC time of day (base time is midnight).
    let fps = args.fps;
    let full_flash = args.full_flash;
    vcaps
        .static_pad("src")
        .unwrap()
        .add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(buffer) = info.buffer_mut() {
                if let Some(pts) = buffer.pts() {
                    draw_marks(
                        buffer.make_mut(),
                        &video_info,
                        pts.nseconds(),
                        period_ns,
                        fps,
                        full_flash,
                    );
                }
            }
            gst::PadProbeReturn::Ok
        });
}

/// Parameter sets and NAL length size from an `avcC` record (codec_data).
fn avcc_parameter_sets(avcc: &[u8]) -> Option<(Vec<Vec<u8>>, usize)> {
    let length_size = usize::from(*avcc.get(4)? & 0x03) + 1;
    let mut sets = Vec::new();
    let mut i = 5;
    for _ in 0..2 {
        // Number of SPS (5 bits), then of PPS.
        let count = *avcc.get(i)? & if sets.is_empty() { 0x1F } else { 0xFF };
        i += 1;
        for _ in 0..count {
            let len = usize::from(u16::from_be_bytes([*avcc.get(i)?, *avcc.get(i + 1)?]));
            sets.push(avcc.get(i + 2..i + 2 + len)?.to_vec());
            i += 2 + len;
        }
    }
    Some((sets, length_size))
}

/// Two appsinks feeding the core's MPEG-TS muxer, as the app does: video
/// (H.264 AVC, the SEI added by the muxer) and audio (raw AAC or Opus
/// packets). The host clock of the muxer is the pipeline's realtime clock:
/// PTS = base + running time, PCR from the clock's time at muxing. Both
/// sinks sync on the clock, so frames leave at their capture time like a
/// camera's.
fn core_ts_sinks(
    args: &Args,
    base_ns: u64,
    stamp: Arc<StampClock>,
    sender: Arc<SrtSender>,
) -> (gst_app::AppSink, gst_app::AppSink) {
    let audio = if args.audio_codec == "opus" {
        ts::AudioCodec::Opus
    } else {
        ts::AudioCodec::Aac
    };
    let muxer = Arc::new(std::sync::Mutex::new(Muxer::new(
        ts::VideoCodec::H264,
        Some(audio),
        ts::AudioCodec::pcr_delay_ns(Some(audio)),
    )));
    let clock_offset_ns = ms_to_ns(args.clock_offset_ms);
    let synced = !args.unsynced;
    let uncertainty_us = if synced {
        args.uncertainty_us
    } else {
        sei::UNCERTAINTY_UNKNOWN
    };
    let tai = stamp.tai;
    let frame_counter = AtomicU32::new(0);
    let running = move |sink: &gst_app::AppSink, sample: &gst::Sample| -> Option<(i64, i64)> {
        let pts = sample.buffer()?.pts()?;
        let running = sample
            .segment()
            .and_then(|s| s.downcast_ref::<gst::ClockTime>()?.to_running_time(pts))
            .unwrap_or(pts);
        let now = sink.clock()?.time().nseconds() as i64;
        Some((base_ns as i64 + running.nseconds() as i64, now))
    };

    let vsink = gst_app::AppSink::builder().sync(true).build();
    let (vmux, vsender) = (muxer.clone(), sender.clone());
    let mut format_set = false;
    vsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let mut mux = vmux.lock().unwrap();
                if !format_set {
                    let avcc = sample
                        .caps()
                        .and_then(|c| c.structure(0)?.get::<gst::Buffer>("codec_data").ok())
                        .and_then(|b| b.map_readable().ok().map(|m| m.to_vec()));
                    if let Some((sets, length_size)) = avcc.as_deref().and_then(avcc_parameter_sets)
                    {
                        format_set = mux
                            .set_video_format(sets.iter().map(|s| s.as_slice()), length_size)
                            .is_ok();
                    }
                }
                let (Some(buffer), Some((capture_ns, now_ns))) =
                    (sample.buffer(), running(sink, &sample))
                else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                let Ok(map) = buffer.map_readable() else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                let fields = Fields {
                    capture_ns: (stamp.stamp(capture_ns) + clock_offset_ns).max(0) as u64,
                    synced,
                    tai,
                    frame_counter: frame_counter.fetch_add(1, Ordering::Relaxed),
                    uncertainty_us,
                };
                let frame = ts::VideoFrame {
                    data: &map,
                    keyframe: !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT),
                    pts_ns: capture_ns,
                    sei: Some(fields),
                };
                mux.write_video(&frame, now_ns, &mut |data, content| {
                    let capture = if content == ts::Content::Video {
                        capture_ns
                    } else {
                        0
                    };
                    let _ = vsender.send(data, capture, 0);
                });
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    let asink = gst_app::AppSink::builder().sync(true).build();
    let mut audio_format_set = false;
    asink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let mut mux = muxer.lock().unwrap();
                if !audio_format_set {
                    audio_format_set = mux
                        .set_audio_format(AUDIO_RATE as u32, AUDIO_CHANNELS as u8)
                        .is_ok();
                }
                let (Some(buffer), Some((pts_ns, now_ns))) =
                    (sample.buffer(), running(sink, &sample))
                else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                if let Ok(map) = buffer.map_readable() {
                    mux.write_audio(&map, pts_ns, now_ns, &mut |data, _| {
                        let _ = sender.send(data, 0, 0);
                    });
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    (vsink, asink)
}

fn run(args: &Args) -> Result<()> {
    gst::init()?;

    // The realtime system clock as pipeline clock: clock time is Unix ns.
    let clock = gst::SystemClock::obtain();
    clock.set_property("clock-type", gst::ClockType::Realtime);
    let now_ns = clock.time().nseconds();
    let base_ns = now_ns - now_ns % DAY_NS;

    let stamp = StampClock::start(args.system_clock);
    if !args.system_clock {
        // Stamps are only comparable once the discipline agrees with the pools.
        // This takes several seconds after start; say so, or a quiet start
        // looks like a sender that never connects.
        println!("waiting for the clock to synchronize (up to 90 s, --system-clock skips this)");
        let t0 = Instant::now();
        while !stamp.healthy() && t0.elapsed() < Duration::from_secs(90) {
            std::thread::sleep(Duration::from_millis(250));
        }
        stamp.refresh();
    }
    println!("{}", stamp.line());
    let rust_srt = if args.rust_srt && !args.local {
        let mut cfg = CallerConfig::from_url(&args.url).map_err(|e| anyhow!(e))?;
        cfg.latency_ms = args.latency_ms as u16;
        if args.streamid.is_some() {
            cfg.stream_id = args.streamid.clone();
        }
        let sender = Arc::new(
            SrtSender::start(cfg, SenderOptions::default(), Arc::new(PrintSrtEvents))
                .map_err(|e| anyhow!(e))?,
        );
        let t0 = Instant::now();
        while !sender.is_connected() && t0.elapsed() < Duration::from_secs(6) {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !sender.is_connected() {
            bail!("SRT: no connection to {}", args.url);
        }
        Some(sender)
    } else {
        None
    };
    let pipeline = build(args, base_ns, stamp.clone(), rust_srt.clone())?;
    pipeline.use_clock(Some(&clock));
    pipeline.set_start_time(gst::ClockTime::NONE);
    pipeline.set_base_time(gst::ClockTime::from_nseconds(base_ns));

    println!(
        "{}: {}x{} @ {} fps to {} (latency {} ms), phase {} ms, clock offset {} ms, {}",
        args.name,
        args.width,
        args.height,
        args.fps,
        args.url,
        args.latency_ms,
        args.phase_ms,
        args.clock_offset_ms,
        if args.unsynced { "unsynced" } else { "synced" },
    );
    pipeline.set_state(gst::State::Paused)?;
    // Go to PLAYING 20 ms before a whole second, so the sources start just
    // before their grid and the alignment shift stays small.
    let in_second = clock.time().nseconds() % SECOND_NS;
    let lead = 20_000_000;
    let wait = (2 * SECOND_NS - in_second - lead) % SECOND_NS;
    std::thread::sleep(Duration::from_nanos(wait));
    pipeline.set_state(gst::State::Playing)?;

    let bus = pipeline.bus().unwrap();
    let started = Instant::now();
    let mut eos_sent = false;
    let mut last_refresh = Instant::now();
    let mut latency_printed = false;
    let result = loop {
        if args.local && !latency_printed && started.elapsed() >= Duration::from_secs(2) {
            // Sinks render at running time plus this: the beep and the flash
            // leave the speakers and the screen this much after their second.
            latency_printed = true;
            let mut q = gst::query::Latency::new();
            if pipeline.query(&mut q) {
                let (live, min, max) = q.result();
                println!(
                    "local playout latency {} ms (live {live}, max {:?})",
                    min.mseconds(),
                    max
                );
            }
        }
        if last_refresh.elapsed() >= Duration::from_secs(10) {
            last_refresh = Instant::now();
            stamp.refresh();
            println!("{}", stamp.line());
        }
        if let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(200)) {
            match msg.view() {
                gst::MessageView::Error(err) => {
                    break Err(anyhow!(
                        "{}: {} ({})",
                        err.src()
                            .map(|s| s.path_string().to_string())
                            .unwrap_or_default(),
                        err.error(),
                        err.debug().unwrap_or_default()
                    ));
                }
                gst::MessageView::Eos(_) => break Ok(()),
                _ => {}
            }
        }
        if let Some(limit) = args.duration_s {
            let elapsed = started.elapsed();
            if !eos_sent && elapsed >= Duration::from_secs(limit) {
                pipeline.send_event(gst::event::Eos::new());
                eos_sent = true;
            } else if eos_sent && elapsed >= Duration::from_secs(limit + 3) {
                break Ok(());
            }
        }
    };
    // srtsink can block forever in shutdown once the listener is gone.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(3));
        eprintln!("shutdown timed out");
        std::process::exit(0);
    });
    pipeline.set_state(gst::State::Null)?;
    if let Some(sender) = rust_srt {
        println!("rust srt: {:?}", sender.stats());
        sender.close();
    }
    result
}

/// Prints the Rust SRT caller's events.
struct PrintSrtEvents;

impl SrtObserver for PrintSrtEvents {
    fn on_event(&self, event: SrtEvent) {
        println!("rust srt: {event:?}");
    }

    fn on_frame_sent(&self, _capture_ns: i64, _enqueued_ns: i64, _sent_ns: i64, _last: bool) {}
}

fn main() -> Result<()> {
    let args = parse_args(std::env::args().skip(1))?;
    run(&args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sei_nal_is_the_core_timestamp_with_start_code() {
        let fields = Fields {
            capture_ns: 0x0000_0001_0000_0000,
            synced: true,
            tai: true,
            frame_counter: 0,
            uncertainty_us: 0,
        };
        let nal = sei_nal(&fields);
        assert_eq!(&nal[..5], &[0, 0, 0, 1, 6]);
        assert_eq!(sei::parse_nal_unit(&nal[4..]), Some((Codec::H264, fields)));
    }

    #[test]
    fn inserts_after_parameter_sets_before_slice() {
        let au = [
            0, 0, 0, 1, 0x09, 0xF0, // AUD
            0, 0, 0, 1, 0x67, 0xAA, // SPS
            0, 0, 1, 0x68, 0xBB, // PPS, 3-byte start code
            0, 0, 0, 1, 0x65, 0xCC, // IDR slice
        ];
        let nal = [0, 0, 0, 1, 0x06, 0x80];
        let out = insert_before_first_slice(&au, &nal).unwrap();
        assert_eq!(&out[..17], &au[..17]);
        assert_eq!(&out[17..23], &nal);
        assert_eq!(&out[23..], &au[17..]);
    }

    #[test]
    fn grid_points() {
        // 30 fps, no phase: 1/30 s steps, exact on whole seconds.
        assert_eq!(next_grid_point(5 * SECOND_NS, 0, 30), 5 * SECOND_NS);
        assert_eq!(
            next_grid_point(5 * SECOND_NS + 1, 0, 30),
            5 * SECOND_NS + 33_333_333
        );
        assert_eq!(
            next_grid_point(5 * SECOND_NS + 990_000_000, 0, 30),
            6 * SECOND_NS
        );
        // 25 fps, 10 ms phase.
        assert_eq!(
            next_grid_point(5 * SECOND_NS + 15_000_000, 10_000_000, 25),
            5 * SECOND_NS + 50_000_000
        );
        assert_eq!(
            next_grid_point(5 * SECOND_NS + 5_000_000, 10_000_000, 25),
            5 * SECOND_NS + 10_000_000
        );
        // Whole seconds.
        assert_eq!(
            next_grid_point(5 * SECOND_NS + 980_000_000, 0, 1),
            6 * SECOND_NS
        );
    }

    #[test]
    fn beep_only_at_start_of_second() {
        gst::init().unwrap();
        // 10 ms of stereo S16 at 48 kHz, all samples 1.
        let mut buffer = gst::Buffer::from_mut_slice(vec![1u8; 480 * 4]);
        // Starts 45 ms into a second: 5 ms of beep, then silence.
        gate_beep(buffer.get_mut().unwrap(), 7 * SECOND_NS + 45_000_000);
        let map = buffer.map_readable().unwrap();
        assert!(map[..240 * 4].iter().all(|&b| b == 1));
        assert!(map[240 * 4..].iter().all(|&b| b == 0));
    }

    #[test]
    fn every_fifth_second_beeps_longer() {
        gst::init().unwrap();
        // 10 ms buffers 100 ms into second 10 (marked) and second 11.
        for (second, kept) in [(10u64, true), (11, false)] {
            let mut buffer = gst::Buffer::from_mut_slice(vec![1u8; 480 * 4]);
            gate_beep(buffer.get_mut().unwrap(), second * SECOND_NS + 100_000_000);
            let map = buffer.map_readable().unwrap();
            assert_eq!(map.iter().all(|&b| b == 1), kept, "second {second}");
        }
    }

    #[test]
    fn parameter_sets_from_avcc() {
        let avcc = [
            1, 0x64, 0, 0x1F, 0xFF, 0xE1, 0, 3, 0x67, 0x64, 0x00, 1, 0, 2, 0x68, 0xEE,
        ];
        let (sets, length_size) = avcc_parameter_sets(&avcc).unwrap();
        assert_eq!(length_size, 4);
        assert_eq!(sets, [vec![0x67, 0x64, 0x00], vec![0x68, 0xEE]]);
        assert!(avcc_parameter_sets(&avcc[..10]).is_none());
    }

    #[test]
    fn no_slice_no_insert() {
        assert!(insert_before_first_slice(&[0, 0, 0, 1, 0x67, 1], &[0, 0, 0, 1, 6]).is_none());
    }

    #[test]
    fn parses_arguments() {
        let args = parse_args(
            [
                "srt://192.0.2.1:9000?streamid=a",
                "--fps",
                "25",
                "--phase-ms",
                "10",
                "--clock-offset-ms",
                "-5",
                "--unsynced",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(args.url, "srt://192.0.2.1:9000?streamid=a");
        assert_eq!(args.fps, 25);
        assert_eq!(args.phase_ms, 10.0);
        assert_eq!(args.clock_offset_ms, -5.0);
        assert!(args.unsynced);
        assert!(parse_args(["--fps", "24"].map(String::from)).is_err());
    }
}
