//! sync-probe: receives TallyCam SRT streams and measures glass-to-X
//! latency and A/V sync from the LCP capture timestamp SEI.
//!
//! Receive times are taken on this machine's clock disciplined by the
//! shared core (the same chrony-like discipline the phone runs, against the
//! same pools), so sender and receiver are on one TAI timescale. The
//! receiver's own clock health is printed alongside; `--system-clock` uses
//! the OS clock instead (only meaningful when chrony runs here).
//!
//! Views, per stream and second (p50/p95/max, ms):
//! - glass->srt: capture to the moment srtsrc hands the SRT payload to the
//!   application. The TS is parsed in the tool (no tsdemux or parser), so
//!   this is the network plus SRT buffer (TSBPD) plus everything on the
//!   sender. Audio is mapped to capture time through the video PTS.
//! - glass->demux: after tsdemux and h264parse or h265parse (appsink, sync=false).
//! - glass->playout: when a syncing sink renders the frame (appsink,
//!   sync=true), as a receiver that plays out on its clock would.
//!
//! It also prints the PTS lead over the PCR (what a TS receiver must buffer)
//! and, with `--av`, the A/V offset of flash/beep test events.

mod av;
mod timing;
mod ts;

use anyhow::{bail, Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tallycam_core::clock::driver::{
    default_host_clock, DisciplineDriver, DriverOptions, HostClock, StdResolver, SystemClockDriver,
};
use tallycam_core::clock::leap::{self, DynamicLeap};
use tallycam_core::clock::{ClockConfig, ClockSource, SnapshotCell};
use tallycam_core::protocol::ClockHealthLevel;
use tallycam_core::sei::{self, Fields};

const USAGE: &str = "\
Usage: sync-probe [OPTIONS] [PORT ...]          (default port 9100)

  --latency-ms MS        SRT latency of the listener (default 120); SRT uses
                         the larger of the sender and receiver latency
  --raw                  only the SRT-layer view (no tsdemux)
  --no-playout           no syncing (playout) view
  --tsdemux-latency MS   tsdemux latency property (default: GStreamer's, 700)
  --av                   decode and report flash/beep A/V offsets
  --beep-threshold X     sample level (0..1) that counts as the beep (default 0.05)
  --system-clock         use this machine's OS clock instead of the core discipline
  --wait-sync            do not start receiving until the clock is healthy
                         with a max error below 10 ms (frames say the sender's: unc)
  --duration-s S         stop after S seconds and print a summary
  --frames-csv PREFIX    write every stamped frame to PREFIX-<port>.csv (capture
                         and arrival TAI) for frame timing analysis
  --analyze CSV          print the frame timing of a saved CSV and exit
  --quiet                only the summary
";

struct Args {
    ports: Vec<u16>,
    latency_ms: i32,
    raw_only: bool,
    playout: bool,
    tsdemux_latency: Option<i32>,
    av: bool,
    beep_threshold: f32,
    system_clock: bool,
    wait_sync: bool,
    duration_s: Option<u64>,
    quiet: bool,
    frames_csv: Option<String>,
}

fn parse_args() -> Result<Args> {
    let mut a = Args {
        ports: Vec::new(),
        latency_ms: 120,
        raw_only: false,
        playout: true,
        tsdemux_latency: None,
        av: false,
        beep_threshold: 0.05,
        system_clock: false,
        wait_sync: false,
        duration_s: None,
        quiet: false,
        frames_csv: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().with_context(|| format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--latency-ms" => a.latency_ms = value(&arg)?.parse().context("--latency-ms")?,
            "--raw" => a.raw_only = true,
            "--no-playout" => a.playout = false,
            "--tsdemux-latency" => {
                a.tsdemux_latency = Some(value(&arg)?.parse().context("--tsdemux-latency")?)
            }
            "--av" => a.av = true,
            "--beep-threshold" => {
                a.beep_threshold = value(&arg)?.parse().context("--beep-threshold")?
            }
            "--system-clock" => a.system_clock = true,
            "--wait-sync" => a.wait_sync = true,
            "--duration-s" => a.duration_s = Some(value(&arg)?.parse().context("--duration-s")?),
            "--quiet" => a.quiet = true,
            "--frames-csv" => a.frames_csv = Some(value(&arg)?),
            other => a.ports.push(
                other
                    .parse()
                    .with_context(|| format!("bad argument {other}\n\n{USAGE}"))?,
            ),
        }
    }
    if a.ports.is_empty() {
        a.ports.push(9100);
    }
    Ok(a)
}

// ---------------------------------------------------------------------------
// Receiver clock

/// This machine's clock on the TAI timescale: the core discipline (or the
/// OS clock), read lock free.
struct RxClock {
    host: Arc<dyn HostClock>,
    cell: Arc<SnapshotCell>,
    source: Arc<dyn ClockSource>,
}

impl RxClock {
    fn start(system: bool) -> Self {
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
        RxClock { host, cell, source }
    }

    /// TAI ns now; 0 before the source published a mapping.
    #[inline]
    fn now_tai(&self) -> i64 {
        let h = self.host.now_ns();
        self.cell.load().map_or(0, |s| s.tai_ns(h))
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
        let h = self.host.now_ns();
        let wall_ms = self.cell.load().map_or(f64::NAN, |snap| {
            (snap.utc_ns(h) - self.host.wall_utc_ns()) as f64 / 1e6
        });
        format!(
            "rx clock: {:?} synced {} est {:.2} ms max {:.2} ms sources {}/{} ref {} | disciplined - OS clock {:+.1} ms",
            s.health,
            s.synced,
            s.est_error_us as f64 / 1e3,
            s.max_error_us as f64 / 1e3,
            s.combined + s.selected,
            s.total,
            s.reference_name.as_deref().unwrap_or("-"),
            wall_ms,
        )
    }
}

fn capture_tai(f: &Fields) -> i64 {
    if f.tai {
        f.capture_ns as i64
    } else {
        leap::utc_to_tai_ns(f.capture_ns as i64, DynamicLeap::default())
    }
}

/// Signed difference of two 33-bit 90 kHz timestamps, in ns.
fn pts_diff_ns(a: u64, b: u64) -> i64 {
    const WRAP: i64 = 1 << 33;
    let mut d = (a as i64 - b as i64).rem_euclid(WRAP);
    if d >= WRAP / 2 {
        d -= WRAP;
    }
    d * 100_000 / 9
}

// ---------------------------------------------------------------------------
// Statistics

#[derive(Default, Clone)]
struct Series {
    second: Vec<f64>,
    all: Vec<f64>,
}

impl Series {
    fn push(&mut self, v: f64) {
        self.second.push(v);
        self.all.push(v);
    }

    fn take_second(&mut self) -> String {
        let s = describe(&mut self.second, false);
        self.second.clear();
        s
    }

    fn total(&mut self) -> String {
        describe(&mut self.all, true)
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i]
}

fn describe(v: &mut [f64], long: bool) -> String {
    if v.is_empty() {
        return "-".into();
    }
    v.sort_by(f64::total_cmp);
    if long {
        format!(
            "p50 {:.1} p95 {:.1} p99 {:.1} min {:.1} max {:.1} (n {})",
            percentile(v, 0.5),
            percentile(v, 0.95),
            percentile(v, 0.99),
            v[0],
            v[v.len() - 1],
            v.len()
        )
    } else {
        format!(
            "{:.1}/{:.1}/{:.1}",
            percentile(v, 0.5),
            percentile(v, 0.95),
            v[v.len() - 1]
        )
    }
}

#[derive(Default)]
struct StreamStats {
    frames: u64,
    stamped: u64,
    keyframes: u64,
    lost_frames: u64,
    total_lost_frames: u64,
    cc_errors: u64,
    synced: bool,
    uncertainty_us: Option<u32>,
    last_counter: Option<u32>,
    last_capture: Option<i64>,
    /// Latest (video PTS, capture TAI) pair, mapping PTS to capture time.
    pts_map: Option<(u64, i64)>,
    srt_video: Series,
    srt_audio: Series,
    demux: Series,
    playout: Series,
    interval: Series,
    pcr_lead_video: Series,
    pcr_lead_audio: Series,
    /// Change of (capture time - PTS) from frame to frame: PTS jitter
    /// against the capture clock.
    pts_jitter: Series,
    av_offset: Series,
    flash_to_second: Series,
    beep_to_second: Series,
    /// Long (marked) beeps to their 5-second mark: no whole-second ambiguity.
    beep_to_mark: Series,
    recent_captures: Vec<i64>,
    /// --av: video PTS as pushed to the decoder (ns) -> capture TAI.
    av_pts: VecDeque<(u64, i64)>,
    /// --av: capture TAI minus decoder PTS (ns) of the latest video frame.
    av_capture_offset: Option<i64>,
    /// --av: the first PTS seen, 90 kHz; decoder PTS count from it.
    av_base90: Option<u64>,
    flash: av::FlashDetector,
    beep: Option<av::BeepDetector>,
    beep_threshold: f32,
    /// --av: loudest decoded sample this second, to set --beep-threshold.
    audio_peak: f32,
    pairer: av::Pairer,
    timing: timing::FrameTiming,
}

impl StreamStats {
    fn on_event(&mut self, event: ts::Event, arrival: i64) {
        match event {
            ts::Event::Video {
                pts90,
                sei,
                keyframe,
                pcr90,
                ..
            } => {
                self.frames += 1;
                self.keyframes += u64::from(keyframe);
                if let (Some(pts), Some(pcr)) = (pts90, pcr90) {
                    self.pcr_lead_video.push(pts_diff_ns(pts, pcr) as f64 / 1e6);
                }
                let Some(f) = sei else { return };
                self.stamped += 1;
                self.synced = f.synced;
                self.uncertainty_us =
                    (f.uncertainty_us != sei::UNCERTAINTY_UNKNOWN).then_some(f.uncertainty_us);
                let capture = capture_tai(&f);
                if arrival != 0 {
                    self.srt_video.push((arrival - capture) as f64 / 1e6);
                }
                if let Some(last) = self.last_capture {
                    self.interval.push((capture - last) as f64 / 1e6);
                }
                if let Some(last) = self.last_counter {
                    let gap = f.frame_counter.wrapping_sub(last);
                    if gap > 1 && gap < 1000 {
                        self.lost_frames += u64::from(gap - 1);
                        self.total_lost_frames += u64::from(gap - 1);
                    }
                }
                self.timing.push(
                    f.frame_counter,
                    capture,
                    arrival,
                    self.uncertainty_us,
                    f.synced,
                );
                self.last_counter = Some(f.frame_counter);
                self.last_capture = Some(capture);
                if let Some(pts) = pts90 {
                    let pts_ns = self.av_pts_ns(pts);
                    self.av_capture_offset = Some(capture - pts_ns as i64);
                    self.av_pts.push_back((pts_ns, capture));
                    if self.av_pts.len() > 120 {
                        self.av_pts.pop_front();
                    }
                    if let Some((p0, c0)) = self.pts_map {
                        let d = (capture - c0) - pts_diff_ns(pts, p0);
                        self.pts_jitter.push(d as f64 / 1e6);
                    }
                    self.pts_map = Some((pts, capture));
                }
                self.recent_captures.push(capture);
                if self.recent_captures.len() > 240 {
                    self.recent_captures.drain(..120);
                }
            }
            ts::Event::Audio { pts90, pcr90 } => {
                let Some(pts) = pts90 else { return };
                if let Some(pcr) = pcr90 {
                    self.pcr_lead_audio.push(pts_diff_ns(pts, pcr) as f64 / 1e6);
                }
                if let (Some((vpts, vcap)), true) = (self.pts_map, arrival != 0) {
                    let capture = vcap + pts_diff_ns(pts, vpts);
                    self.srt_audio.push((arrival - capture) as f64 / 1e6);
                }
            }
            ts::Event::Pcr { .. } => {}
            ts::Event::Discontinuity { .. } => self.cc_errors += 1,
            ts::Event::Pes { .. } => {}
        }
    }

    /// A 90 kHz PTS as decoder PTS in ns: 10 s plus the (wrap-safe) distance
    /// from the first PTS seen.
    fn av_pts_ns(&mut self, pts90: u64) -> u64 {
        let base = *self.av_base90.get_or_insert(pts90);
        (10_000_000_000 + pts_diff_ns(pts90, base)).max(0) as u64
    }

    fn fps(&self) -> Option<i64> {
        let mut v = self.interval.second.clone();
        if v.is_empty() {
            return None;
        }
        v.sort_by(f64::total_cmp);
        let median = v[v.len() / 2];
        (median > 0.0).then(|| (1000.0 / median).round() as i64)
    }
}

/// Phase of a capture time within the frame grid that starts on each whole
/// second, in ns.
fn frame_phase_ns(capture_ns: i64, fps: i64) -> i64 {
    let in_second = capture_ns.rem_euclid(1_000_000_000);
    let phase = ((in_second * fps % 1_000_000_000) + fps / 2) / fps;
    if phase >= 1_000_000_000 / fps {
        0
    } else {
        phase
    }
}

/// For each capture in `other` within the span of `reference`, the distance
/// to the nearest capture in `reference`, ms.
fn nearest_offsets_ms(reference: &[i64], other: &[i64]) -> Vec<f64> {
    let (Some(&first), Some(&last)) = (reference.first(), reference.last()) else {
        return Vec::new();
    };
    other
        .iter()
        .filter(|&&t| t >= first && t <= last)
        .map(|&t| {
            let idx = reference.partition_point(|&r| r < t);
            [idx.checked_sub(1), Some(idx)]
                .into_iter()
                .flatten()
                .filter_map(|i| reference.get(i))
                .map(|&r| (t - r) as f64 / 1e6)
                .min_by(|a, b| a.abs().total_cmp(&b.abs()))
                .unwrap_or(f64::NAN)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Pipeline

fn make(factory: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .build()
        .with_context(|| format!("GStreamer element {factory} is missing"))
}

fn sei_capture(buffer: &gst::BufferRef) -> Option<Fields> {
    buffer
        .iter_meta::<gst_video::video_meta::VideoSeiUserDataUnregisteredMeta>()
        .filter(|meta| meta.uuid() == sei::UUID)
        .find_map(|meta| sei::parse_user_data(meta.data()))
}

#[derive(Clone)]
struct Ctx {
    port: u16,
    clock: Arc<RxClock>,
    stats: Arc<Mutex<StreamStats>>,
}

/// Records glass-to-demux (`playout` false, sync=false) or glass-to-playout
/// (`playout` true, sync=true) of every stamped frame.
fn timing_sink(ctx: &Ctx, playout: bool) -> gst::Element {
    let clock = ctx.clock.clone();
    let stats = ctx.stats.clone();
    let sink = gst_app::AppSink::builder().sync(playout).build();
    if playout {
        // Render on time and never drop for lateness: we want to see it.
        sink.set_property("max-lateness", -1i64);
    }
    sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let arrival = clock.now_tai();
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let Some(buffer) = sample.buffer() else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                let Some(f) = sei_capture(buffer) else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                let capture = capture_tai(&f);
                let mut s = stats.lock().unwrap();
                if arrival != 0 {
                    let ms = (arrival - capture) as f64 / 1e6;
                    if playout {
                        s.playout.push(ms);
                    } else {
                        s.demux.push(ms);
                    }
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    sink.upcast()
}

fn report_pair(s: &mut StreamStats, flash: i64, beep: i64) {
    let offset = (beep - flash) as f64 / 1e6;
    println!(
        "  A/V event: flash {:+.1} ms, beep {:+.1} ms from the whole second; audio - video {offset:+.1} ms",
        av::to_whole_second(flash) as f64 / 1e6,
        av::to_whole_second(beep) as f64 / 1e6,
    );
    s.av_offset.push(offset);
}

/// Mean brightness of decoded frames, for the flash detector.
fn luma_sink(ctx: &Ctx) -> gst::Element {
    let stats = ctx.stats.clone();
    let port = ctx.port;
    let sink = gst_app::AppSink::builder()
        .sync(false)
        .caps(
            &gst::Caps::builder("video/x-raw")
                .field("format", "GRAY8")
                .build(),
        )
        .build();
    sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                let Ok(info) = gst_video::VideoInfo::from_caps(caps) else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                let Some(pts) = buffer.pts() else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                let Ok(map) = buffer.map_readable() else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                let stride = info.stride()[0] as usize;
                let cells =
                    av::grid_means(&map, info.width() as usize, info.height() as usize, stride);
                let mut s = stats.lock().unwrap();
                let capture = s
                    .av_pts
                    .iter()
                    .find(|(p, _)| *p == pts.nseconds())
                    .map(|&(_, c)| c);
                if let Some(capture) = capture {
                    if let Some(t) = s.flash.push(cells, capture) {
                        // One line per flash, to pair flashes across cameras.
                        let brightest = cells.iter().copied().fold(0.0, f64::max);
                        println!(
                            "flash port {port} capture_tai_ns {t} peak_cell_luma {brightest:.0}"
                        );
                        s.flash_to_second.push(av::to_whole_second(t) as f64 / 1e6);
                        if let Some((f, b)) = s.pairer.flash(t) {
                            report_pair(&mut s, f, b);
                        }
                    }
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    sink.upcast()
}

/// Decoded mono audio, for the beep detector. Sample times come from the
/// demuxed PTS mapped to capture time through the latest video frame.
fn audio_sink(ctx: &Ctx) -> gst::Element {
    let stats = ctx.stats.clone();
    let sink = gst_app::AppSink::builder()
        .sync(false)
        .caps(
            &gst::Caps::builder("audio/x-raw")
                .field("format", "F32LE")
                .field("channels", 1)
                .field("layout", "interleaved")
                .build(),
        )
        .build();
    sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| {
                let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let (Some(buffer), Some(caps)) = (sample.buffer(), sample.caps()) else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                let rate = caps
                    .structure(0)
                    .and_then(|s| s.get::<i32>("rate").ok())
                    .unwrap_or(48_000) as u32;
                let Some(pts) = buffer.pts() else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                let Ok(map) = buffer.map_readable() else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                let samples: Vec<f32> = map
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect();
                let mut s = stats.lock().unwrap();
                let Some(offset) = s.av_capture_offset else {
                    return Ok(gst::FlowSuccess::Ok);
                };
                let first = pts.nseconds() as i64 + offset;
                let peak = samples.iter().fold(0.0f32, |m, x| m.max(x.abs()));
                s.audio_peak = s.audio_peak.max(peak);
                let threshold = s.beep_threshold;
                let onsets = s
                    .beep
                    .get_or_insert_with(|| {
                        let mut d = av::BeepDetector::new(rate);
                        d.threshold = threshold;
                        d
                    })
                    .push(&samples, first);
                for beep in onsets {
                    let t = beep.onset;
                    s.beep_to_second.push(av::to_whole_second(t) as f64 / 1e6);
                    if beep.length >= av::MARK_BEEP_MIN_NS {
                        // Marked seconds are UTC seconds divisible by 5.
                        let utc = leap::tai_to_utc_ns(t, DynamicLeap::default());
                        s.beep_to_mark
                            .push(av::to_nearest(utc, 5_000_000_000) as f64 / 1e6);
                    }
                    if let Some((f, b)) = s.pairer.beep(t) {
                        report_pair(&mut s, f, b);
                    }
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );
    sink.upcast()
}

fn add_chain(pipeline: &gst::Pipeline, elements: &[&gst::Element]) -> Result<()> {
    pipeline.add_many(elements.iter().copied())?;
    gst::Element::link_many(elements.iter().copied())?;
    for e in elements {
        e.sync_state_with_parent()?;
    }
    Ok(())
}

fn link_demux_pad(
    pipeline: &gst::Pipeline,
    pad: &gst::Pad,
    ctx: &Ctx,
    playout: bool,
) -> Result<()> {
    let caps = pad.current_caps().unwrap_or_else(|| pad.query_caps(None));
    let Some(name) = caps.structure(0).map(|s| s.name().to_string()) else {
        return Ok(());
    };
    let q = make("queue")?;
    match name.as_str() {
        "video/x-h264" | "video/x-h265" => {
            let parse = make(if name == "video/x-h264" {
                "h264parse"
            } else {
                "h265parse"
            })?;
            let tee = make("tee")?;
            add_chain(pipeline, &[&q, &parse, &tee])?;
            let q1 = make("queue")?;
            let demux_sink = timing_sink(ctx, false);
            add_chain(pipeline, &[&q1, &demux_sink])?;
            tee.link(&q1)?;
            if playout {
                let q2 = make("queue")?;
                let sink = timing_sink(ctx, true);
                add_chain(pipeline, &[&q2, &sink])?;
                tee.link(&q2)?;
            }
        }
        _ => return Ok(()),
    }
    pad.link(&q.static_pad("sink").unwrap())?;
    println!("port {}: demuxing {name}", ctx.port);
    Ok(())
}

/// Decoders fed from the tool's own TS parser (`--av`): the flash and beep
/// are timed by the PTS in the stream, not by a demuxer's re-timestamping.
fn build_av(pipeline: &gst::Pipeline, ctx: &Ctx) -> Result<(gst_app::AppSrc, gst_app::AppSrc)> {
    let src = |caps: gst::Caps| {
        gst_app::AppSrc::builder()
            .caps(&caps)
            .format(gst::Format::Time)
            .is_live(false)
            .block(false)
            .max_bytes(0)
            .build()
    };
    // The video parser and decoder (H.264 or HEVC) are linked in once the
    // PMT is known, see `link_video_decoder`.
    let video_src = src(gst::Caps::new_any());
    let video_convert = make("videoconvert")?;
    let luma = luma_sink(ctx);
    add_chain(pipeline, &[video_src.upcast_ref()])?;
    add_chain(pipeline, &[&video_convert, &luma])?;
    // The audio caps are set from the PMT once it is known (`audio_caps`).
    let audio_src = src(gst::Caps::new_any());
    // decodebin picks the decoder (AAC or Opus), as Strom's SRT input does.
    let decode = make("decodebin")?;
    let convert = make("audioconvert")?;
    let resample = make("audioresample")?;
    let sink = audio_sink(ctx);
    add_chain(pipeline, &[audio_src.upcast_ref(), &decode])?;
    add_chain(pipeline, &[&convert, &resample, &sink])?;
    let convert_weak = convert.downgrade();
    let port = ctx.port;
    decode.connect_pad_added(move |_, pad| {
        let Some(convert) = convert_weak.upgrade() else {
            return;
        };
        let name = pad
            .current_caps()
            .and_then(|c| c.structure(0).map(|s| s.name().to_string()))
            .unwrap_or_default();
        if !name.starts_with("audio/x-raw") {
            return;
        }
        if let Err(e) = pad.link(&convert.static_pad("sink").unwrap()) {
            eprintln!("port {port}: audio decoder pad: {e:?}");
        }
    });
    Ok((video_src, audio_src))
}

/// Sets the `--av` video caps for `codec` and links its parser and software
/// decoder between the video appsrc and the videoconvert after it.
fn link_video_decoder(video_src: &gst_app::AppSrc, codec: ts::VideoCodec) -> Result<String> {
    let (name, parse, decode) = match codec {
        ts::VideoCodec::H264 => ("video/x-h264", "h264parse", "avdec_h264"),
        ts::VideoCodec::Hevc => ("video/x-h265", "h265parse", "avdec_h265"),
    };
    let caps = gst::Caps::builder(name)
        .field("stream-format", "byte-stream")
        .field("alignment", "au")
        .build();
    video_src.set_caps(Some(&caps));
    let pipeline = video_src
        .parent()
        .and_then(|p| p.downcast::<gst::Pipeline>().ok())
        .context("video appsrc outside the pipeline")?;
    let convert = pipeline
        .iterate_elements()
        .into_iter()
        .flatten()
        .find(|e| e.factory().is_some_and(|f| f.name() == "videoconvert"))
        .context("no videoconvert")?;
    let parse = make(parse)?;
    let decode = make(decode)?;
    pipeline.add_many([&parse, &decode])?;
    gst::Element::link_many([video_src.upcast_ref(), &parse, &decode, &convert])?;
    parse.sync_state_with_parent()?;
    decode.sync_state_with_parent()?;
    Ok(caps.to_string())
}

/// Caps of the audio elementary stream as pushed to the decoder.
fn audio_caps(codec: ts::AudioCodec) -> Option<gst::Caps> {
    match codec {
        ts::AudioCodec::Aac => Some(
            gst::Caps::builder("audio/mpeg")
                .field("mpegversion", 4)
                .field("stream-format", "adts")
                .build(),
        ),
        // As tsdemux describes Opus in MPEG-TS: no OpusHead, mapping family 0.
        ts::AudioCodec::Opus { channels } => Some(
            gst::Caps::builder("audio/x-opus")
                .field("rate", 48_000i32)
                .field("channels", i32::from(channels))
                .field("channel-mapping-family", 0i32)
                .build(),
        ),
        ts::AudioCodec::Other(_) => None,
    }
}

fn build_stream(args: &Args, ctx: Ctx) -> Result<gst::Pipeline> {
    let port = ctx.port;
    let pipeline = gst::Pipeline::with_name(&format!("probe-{port}"));
    let src = gst::ElementFactory::make("srtsrc")
        .name("src")
        .property("uri", format!("srt://:{port}?mode=listener"))
        .property("latency", args.latency_ms)
        .property("keep-listening", true)
        .build()
        .context("srtsrc missing")?;
    let tee = make("tee")?;
    tee.set_property("allow-not-linked", true);

    // --av: decode the elementary streams with the PTS exactly as sent.
    let av = if args.av {
        Some(build_av(&pipeline, &ctx)?)
    } else {
        None
    };

    // SRT-layer view: the raw appsink runs in srtsrc's streaming thread
    // (no queue), so the arrival time is when srtsrc hands over the payload.
    let raw = gst_app::AppSink::builder().sync(false).build();
    raw.set_property("async", false);
    {
        let clock = ctx.clock.clone();
        let stats = ctx.stats.clone();
        let mut reader = ts::TsReader::new();
        reader.collect_pes = av.is_some();
        raw.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let arrival = clock.now_tai();
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let Some(buffer) = sample.buffer() else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let Ok(map) = buffer.map_readable() else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let mut pes = Vec::new();
                    let audio_codec;
                    let video_codec;
                    {
                        let mut s = stats.lock().unwrap();
                        reader.feed(&map, &mut |e| match e {
                            ts::Event::Pes { video, pts90, es } => {
                                let pts = pts90.map(|p| s.av_pts_ns(p));
                                pes.push((video, pts, es));
                            }
                            e => s.on_event(e, arrival),
                        });
                        audio_codec = reader.audio_codec();
                        video_codec = reader.video_codec();
                    }
                    if let Some((video_src, audio_src)) = &av {
                        for (video, pts, es) in pes {
                            let pts = pts.map(gst::ClockTime::from_nseconds);
                            if video {
                                let Some(codec) = video_codec else { continue };
                                if video_src.caps().is_none_or(|c| c.is_any()) {
                                    match link_video_decoder(video_src, codec) {
                                        Ok(caps) => println!(
                                            "port {port}: video {codec:?}, decoding {caps}"
                                        ),
                                        Err(e) => {
                                            eprintln!(
                                                "port {port}: cannot decode video {codec:?}: {e:#}"
                                            );
                                            continue;
                                        }
                                    }
                                }
                                let mut buffer = gst::Buffer::from_mut_slice(es);
                                buffer.get_mut().unwrap().set_pts(pts);
                                let _ = video_src.push_buffer(buffer);
                                continue;
                            }
                            let Some(codec) = audio_codec else { continue };
                            if audio_src.caps().is_none_or(|c| c.is_any()) {
                                let Some(caps) = audio_caps(codec) else {
                                    eprintln!("port {port}: cannot decode audio {codec:?}");
                                    continue;
                                };
                                println!("port {port}: audio {codec:?}, decoding {caps}");
                                audio_src.set_caps(Some(&caps));
                            }
                            match codec {
                                ts::AudioCodec::Opus { .. } => {
                                    // One buffer per Opus packet, without
                                    // the TS control header.
                                    let Some(packets) = ts::opus_packets(&es) else {
                                        eprintln!("port {port}: bad Opus control header");
                                        continue;
                                    };
                                    for (i, packet) in packets.into_iter().enumerate() {
                                        let mut buffer = gst::Buffer::from_slice(packet.to_vec());
                                        if i == 0 {
                                            buffer.get_mut().unwrap().set_pts(pts);
                                        }
                                        let _ = audio_src.push_buffer(buffer);
                                    }
                                }
                                _ => {
                                    let mut buffer = gst::Buffer::from_mut_slice(es);
                                    buffer.get_mut().unwrap().set_pts(pts);
                                    let _ = audio_src.push_buffer(buffer);
                                }
                            }
                        }
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
    }
    let raw = raw.upcast::<gst::Element>();
    pipeline.add_many([&src, &tee, &raw])?;
    src.link(&tee)?;
    tee.link(&raw)?;

    if args.raw_only {
        return Ok(pipeline);
    }

    let queue = make("queue")?;
    let demux = make("tsdemux")?;
    if let Some(ms) = args.tsdemux_latency {
        demux.set_property("latency", ms);
    }
    pipeline.add_many([&queue, &demux])?;
    gst::Element::link_many([&tee, &queue, &demux])?;

    let weak_pipeline = pipeline.downgrade();
    let playout = args.playout;
    demux.connect_pad_added(move |_, pad| {
        let Some(pipeline) = weak_pipeline.upgrade() else {
            return;
        };
        if let Err(e) = link_demux_pad(&pipeline, pad, &ctx, playout) {
            eprintln!("port {}: {}: {e:#}", ctx.port, pad.name());
        }
    });
    Ok(pipeline)
}

/// Selected receiver-side SRT statistics of the connected caller.
fn srt_stats(pipeline: &gst::Pipeline) -> String {
    let Some(src) = pipeline.by_name("src") else {
        return String::new();
    };
    let stats = src.property::<gst::Structure>("stats");
    let caller = stats
        .get::<gst::glib::ValueArray>("callers")
        .ok()
        .and_then(|a| a.iter().next().and_then(|v| v.get::<gst::Structure>().ok()))
        .unwrap_or(stats);
    let int = |k: &str| {
        caller
            .get::<i64>(k)
            .map(|v| v.to_string())
            .or_else(|_| caller.get::<i32>(k).map(|v| v.to_string()))
            .or_else(|_| caller.get::<u64>(k).map(|v| v.to_string()))
            .unwrap_or_else(|_| "-".into())
    };
    let float = |k: &str| {
        caller
            .get::<f64>(k)
            .map_or("-".into(), |v| format!("{v:.2}"))
    };
    format!(
        "srt: rtt {} ms latency {} ms lost {} dropped {} retrans {} recv {} Mbit/s",
        float("rtt-ms"),
        int("negotiated-latency-ms"),
        int("packets-received-lost"),
        int("packets-received-dropped"),
        int("packets-received-retransmitted"),
        float("receive-rate-mbps"),
    )
}

fn main() -> Result<()> {
    let mut argv = std::env::args().skip(1);
    if argv.next().as_deref() == Some("--analyze") {
        let path = argv.next().context("--analyze needs a CSV path")?;
        let t = timing::FrameTiming::from_csv(std::path::Path::new(&path))?;
        for line in t.report().context("fewer than two seconds of frames")? {
            println!("{line}");
        }
        return Ok(());
    }
    let args = parse_args()?;
    gst::init()?;
    let clock = Arc::new(RxClock::start(args.system_clock));
    println!(
        "receiver clock: {}",
        if args.system_clock {
            "OS clock (--system-clock)"
        } else {
            "core discipline (Strom chrony pools)"
        }
    );
    if args.wait_sync {
        let t0 = Instant::now();
        while !clock.healthy() {
            if t0.elapsed() > Duration::from_secs(300) {
                bail!("clock not healthy after 300 s: {}", clock.line());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        println!("clock healthy after {:.1} s", t0.elapsed().as_secs_f64());
    }
    println!("{}", clock.line());

    let mut streams = Vec::new();
    for &port in &args.ports {
        let timing = match &args.frames_csv {
            Some(prefix) => {
                let path = std::path::PathBuf::from(format!("{prefix}-{port}.csv"));
                println!("port {port}: writing frames to {}", path.display());
                timing::FrameTiming::with_csv(&path)?
            }
            None => timing::FrameTiming::default(),
        };
        let stats = Arc::new(Mutex::new(StreamStats {
            beep_threshold: args.beep_threshold,
            timing,
            ..Default::default()
        }));
        let ctx = Ctx {
            port,
            clock: clock.clone(),
            stats: stats.clone(),
        };
        let pipeline = build_stream(&args, ctx)?;
        pipeline.set_state(gst::State::Playing)?;
        println!(
            "port {port}: listening on srt://:{port} latency {} ms",
            args.latency_ms
        );
        streams.push((port, pipeline, stats));
    }

    let started = Instant::now();
    let mut tick = 0u64;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        tick += 1;
        for (_, pipeline, _) in &streams {
            let bus = pipeline.bus().unwrap();
            while let Some(msg) = bus.pop() {
                match msg.view() {
                    gst::MessageView::Error(err) => {
                        bail!("{}: {} ({:?})", pipeline.name(), err.error(), err.debug())
                    }
                    gst::MessageView::Latency(_) => {
                        let _ = pipeline.recalculate_latency();
                    }
                    _ => {}
                }
            }
        }
        let done = args
            .duration_s
            .is_some_and(|d| started.elapsed() >= Duration::from_secs(d));
        let mut snapshots = Vec::new();
        for (port, pipeline, stats) in &streams {
            let mut guard = stats.lock().unwrap();
            let s = &mut *guard;
            if !args.quiet {
                if tick % 10 == 1 && *port == streams[0].0 {
                    println!("{}", clock.line());
                }
                let phase = match (s.fps(), s.last_capture) {
                    (Some(fps), Some(t)) if fps > 0 => {
                        format!("{:.1}@{fps}", frame_phase_ns(t, fps) as f64 / 1e6)
                    }
                    _ => "-".into(),
                };
                let (srt_v, srt_a) = (s.srt_video.take_second(), s.srt_audio.take_second());
                let mut line = format!(
                    "port {port}: frames {} stamped {} key {} lost {} cc {} synced {} unc {} | glass->srt v {srt_v} a {srt_a}",
                    s.frames,
                    s.stamped,
                    s.keyframes,
                    s.lost_frames,
                    s.cc_errors,
                    s.synced,
                    s.uncertainty_us.map_or("-".into(), |u| format!("{u}us")),
                );
                if !args.raw_only {
                    line += &format!(
                        " | demux {} | playout {}",
                        s.demux.take_second(),
                        s.playout.take_second()
                    );
                }
                line += &format!(
                    " | pts-pcr v {} a {} | interval {} phase {phase}",
                    s.pcr_lead_video.take_second(),
                    s.pcr_lead_audio.take_second(),
                    s.interval.take_second(),
                );
                if args.av {
                    line += &format!(" | audio peak {:.3}", s.audio_peak);
                }
                println!("{line}");
                if tick.is_multiple_of(5) {
                    println!("  {}", srt_stats(pipeline));
                }
            }
            for series in [
                &mut s.srt_video,
                &mut s.srt_audio,
                &mut s.demux,
                &mut s.playout,
                &mut s.pcr_lead_video,
                &mut s.pcr_lead_audio,
                &mut s.interval,
                &mut s.pts_jitter,
                &mut s.av_offset,
                &mut s.flash_to_second,
                &mut s.beep_to_second,
                &mut s.beep_to_mark,
            ] {
                series.second.clear();
            }
            s.audio_peak = 0.0;
            s.frames = 0;
            s.stamped = 0;
            s.keyframes = 0;
            s.lost_frames = 0;
            snapshots.push((*port, s.recent_captures.clone()));
        }
        if !args.quiet {
            if let Some((ref_port, reference)) = snapshots.first() {
                for (port, captures) in snapshots.iter().skip(1) {
                    let mut offsets = nearest_offsets_ms(reference, captures);
                    println!(
                        "  offset {port} vs {ref_port}: ms {}",
                        describe(&mut offsets, false)
                    );
                }
            }
        }
        if done {
            break;
        }
    }

    println!(
        "==== summary after {:.0} s ====",
        started.elapsed().as_secs_f64()
    );
    println!("{}", clock.line());
    for (port, pipeline, stats) in &streams {
        let mut s = stats.lock().unwrap();
        println!(
            "port {port}: lost frames {} cc errors {}",
            s.total_lost_frames, s.cc_errors
        );
        println!("  glass->srt video ms   {}", s.srt_video.total());
        println!("  glass->srt audio ms   {}", s.srt_audio.total());
        if !args.raw_only {
            println!("  glass->demux ms       {}", s.demux.total());
            println!("  glass->playout ms     {}", s.playout.total());
        }
        println!("  pts-pcr video ms      {}", s.pcr_lead_video.total());
        println!("  pts-pcr audio ms      {}", s.pcr_lead_audio.total());
        println!("  capture interval ms   {}", s.interval.total());
        s.timing.flush();
        for line in s.timing.report().unwrap_or_default() {
            println!("  {line}");
        }
        println!("  pts jitter ms         {}", s.pts_jitter.total());
        if args.av {
            println!("  flash to second ms    {}", s.flash_to_second.total());
            println!("  beep to second ms     {}", s.beep_to_second.total());
            println!("  marked beep to 5 s ms {}", s.beep_to_mark.total());
            println!("  audio - video ms      {}", s.av_offset.total());
        }
        println!("  {}", srt_stats(pipeline));
        let _ = pipeline.set_state(gst::State::Null);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pts_wraps() {
        assert_eq!(pts_diff_ns(90_000, 0), 1_000_000_000);
        assert_eq!(pts_diff_ns(0, 90_000), -1_000_000_000);
        assert_eq!(pts_diff_ns(10, (1 << 33) - 80), 1_000_000);
    }

    #[test]
    fn utc_stamps_become_tai() {
        let f = Fields {
            capture_ns: 1_760_000_000_000_000_000,
            synced: true,
            tai: false,
            frame_counter: 0,
            uncertainty_us: 0,
        };
        assert_eq!(capture_tai(&f), 1_760_000_037_000_000_000);
        let t = Fields { tai: true, ..f };
        assert_eq!(capture_tai(&t), 1_760_000_000_000_000_000);
    }

    #[test]
    fn nearest_offsets() {
        let reference = [0, 33_000_000, 66_000_000];
        let other = [5_000_000, 31_000_000, 70_000_000];
        assert_eq!(nearest_offsets_ms(&reference, &other), vec![5.0, -2.0]);
    }

    #[test]
    fn phase_at_30_fps() {
        let second = 1_760_000_000_000_000_000i64;
        let frame7 = second + 7 * 1_000_000_000 / 30 + 3_000_000;
        assert_eq!(frame_phase_ns(frame7, 30), 3_000_000);
        assert_eq!(frame_phase_ns(second, 30), 0);
    }

    #[test]
    fn percentiles() {
        let mut v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(describe(&mut v, false), "51.0/95.0/100.0");
    }
}
