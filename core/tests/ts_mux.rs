//! The MPEG-TS muxer end to end: its output read back with sync-probe's TS
//! reader (the receiver the latency numbers come from), byte layouts that
//! receivers depend on, and real H.264 checked with ffprobe and GStreamer
//! when they are installed.

#[allow(dead_code)]
#[path = "../../tools/sync-probe/src/ts.rs"]
mod probe_ts;

use probe_ts::{Event, TsReader};
use tallycam_core::sei::Fields;
use tallycam_core::ts::{
    annexb, AudioCodec, Content, Muxer, TsClock, VideoCodec, VideoFrame, AUDIO_PID, PACKET_SIZE,
    PMT_PID, VIDEO_PID,
};

const MS: i64 = 1_000_000;
/// A 1280x720 Constrained Baseline SPS and its PPS.
const SPS: &[u8] = &[
    0x67, 0x42, 0xC0, 0x1F, 0xDA, 0x01, 0x40, 0x16, 0xEC, 0x05, 0xA8, 0x08, 0x08, 0x0A, 0x00, 0x00,
    0x03, 0x00, 0x02, 0x00, 0x00, 0x03, 0x00, 0x79, 0x1E, 0x30, 0x65, 0x40,
];
const PPS: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];
/// AAC-LC silence, one channel.
const AAC_SILENCE: &[u8] = &[0x21, 0x10, 0x04, 0x60, 0x8C, 0x1C];

fn length_prefixed(nals: &[&[u8]]) -> Vec<u8> {
    nals.iter()
        .flat_map(|n| {
            (n.len() as u32)
                .to_be_bytes()
                .into_iter()
                .chain(n.iter().copied())
        })
        .collect()
}

fn slice(keyframe: bool) -> Vec<u8> {
    length_prefixed(&[&[if keyframe { 0x65 } else { 0x41 }, 0x88, 0x84, 0x21, 0x43]])
}

fn fields(capture_ns: u64, frame_counter: u32) -> Fields {
    Fields {
        capture_ns,
        synced: true,
        tai: true,
        frame_counter,
        uncertainty_us: 900,
    }
}

/// Collects the output and checks the chunking the SRT sender relies on.
#[derive(Default)]
struct Out {
    bytes: Vec<u8>,
    chunks: Vec<(Content, usize)>,
}

impl Out {
    fn sink(&mut self) -> impl FnMut(&[u8], Content) + '_ {
        |ts, content| {
            assert_eq!(ts.len() % PACKET_SIZE, 0);
            assert!(ts.chunks(PACKET_SIZE).all(|p| p[0] == 0x47));
            self.bytes.extend_from_slice(ts);
            self.chunks.push((content, ts.len() / PACKET_SIZE));
        }
    }

    fn events(&self) -> Vec<Event> {
        let mut reader = TsReader::new();
        reader.collect_pes = true;
        let mut events = Vec::new();
        reader.feed(&self.bytes, &mut |e| events.push(e));
        events
    }

    fn packets(&self, pid: u16) -> Vec<&[u8]> {
        self.bytes
            .chunks(PACKET_SIZE)
            .filter(|p| u16::from(p[1] & 0x1F) << 8 | u16::from(p[2]) == pid)
            .collect()
    }
}

fn h264_muxer(audio: Option<AudioCodec>, pcr_delay_ns: i64) -> Muxer {
    let mut m = Muxer::new(VideoCodec::H264, audio, pcr_delay_ns);
    m.set_video_format([SPS, PPS], 4).unwrap();
    m
}

#[test]
fn video_pts_is_capture_time_and_pcr_runs_delay_behind_mux_time() {
    let mut m = h264_muxer(None, 90 * MS);
    let mut out = Out::default();
    let capture = 5_000 * MS;
    let data = slice(true);
    let frame = VideoFrame {
        data: &data,
        keyframe: true,
        pts_ns: capture,
        sei: None,
    };
    assert!(m.write_video(&frame, capture + 25 * MS, &mut out.sink()));
    let video: Vec<_> = out
        .events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Video {
                pts90,
                dts90,
                pcr90,
                keyframe,
                ..
            } => Some((pts90.unwrap(), dts90, pcr90.unwrap(), keyframe)),
            _ => None,
        })
        .collect();
    assert_eq!(video.len(), 1);
    let (pts, dts, pcr, keyframe) = video[0];
    assert_eq!(pts, TsClock::ticks(capture), "PTS is the capture time");
    assert_eq!(dts, None, "no reordering, no DTS");
    assert!(keyframe);
    assert_eq!(pcr, TsClock::ticks(capture + 25 * MS - 90 * MS));
    assert_eq!(pts - pcr, 65 * 90, "PTS - PCR = delay - time on the phone");
    assert_eq!(m.late_video_frames(), 0);
    assert_eq!(out.chunks[0], (Content::Tables, 2));
    assert_eq!(out.chunks[1], (Content::Video, 1));

    // A frame that took longer than the PCR delay is counted as late.
    let data = slice(false);
    let late = VideoFrame {
        data: &data,
        keyframe: false,
        pts_ns: capture + 33 * MS,
        sei: None,
    };
    assert!(m.write_video(&late, capture + 133 * MS, &mut out.sink()));
    assert_eq!(m.late_video_frames(), 1);
    assert_eq!(out.chunks.len(), 3, "no tables before a P frame");
}

#[test]
fn waits_for_format_keyframe_and_audio() {
    let mut m = Muxer::new(VideoCodec::H264, Some(AudioCodec::Opus), 80 * MS);
    let mut out = Out::default();
    let key = slice(true);
    let p = slice(false);
    let frame = |data, keyframe, t| VideoFrame {
        data,
        keyframe,
        pts_ns: t,
        sei: None,
    };
    assert!(
        !m.write_video(&frame(&key, true, 0), 0, &mut out.sink()),
        "no format"
    );
    assert!(m.set_video_format([SPS], 4).is_err(), "PPS missing");
    m.set_video_format([SPS, PPS], 4).unwrap();
    assert!(
        !m.write_video(&frame(&key, true, 0), 0, &mut out.sink()),
        "no audio format"
    );
    assert!(m.set_audio_format(44_100, 1).is_err(), "Opus needs 48 kHz");
    assert!(m.set_audio_format(48_000, 3).is_err());
    m.set_audio_format(48_000, 1).unwrap();
    assert!(
        !m.write_audio(&[1], 0, 0, &mut out.sink()),
        "before the first video frame"
    );
    assert!(
        !m.write_video(&frame(&p, false, 0), 0, &mut out.sink()),
        "starts on a keyframe"
    );
    assert!(out.bytes.is_empty());
    assert!(m.write_video(&frame(&key, true, 1000 * MS), 1000 * MS, &mut out.sink()));
    assert_eq!(m.start_ns(), Some(1000 * MS));
    assert!(
        !m.write_audio(&[1], 999 * MS, 1000 * MS, &mut out.sink()),
        "audio from before the start"
    );
    assert!(m.write_audio(&[1], 1000 * MS, 1000 * MS, &mut out.sink()));
    // A bad format later stops the video.
    assert!(m.set_video_format([PPS], 4).is_err());
    assert!(!m.write_video(&frame(&p, false, 1033 * MS), 1033 * MS, &mut out.sink()));
}

#[test]
fn audio_pts_is_capture_time_on_the_same_clock() {
    let mut m = h264_muxer(Some(AudioCodec::Aac), 90 * MS);
    m.set_audio_format(48_000, 1).unwrap();
    let mut out = Out::default();
    let start = 7_000 * MS;
    assert!(!m.write_audio(AAC_SILENCE, start - 10 * MS, start, &mut out.sink()));
    let key = slice(true);
    let frame = VideoFrame {
        data: &key,
        keyframe: true,
        pts_ns: start,
        sei: None,
    };
    m.write_video(&frame, start + 20 * MS, &mut out.sink());
    let audio_capture = start + 5 * MS;
    m.write_audio(
        AAC_SILENCE,
        audio_capture,
        audio_capture + 70 * MS,
        &mut out.sink(),
    );
    m.write_audio(
        AAC_SILENCE,
        audio_capture + 21 * MS,
        audio_capture + 21 * MS + 95 * MS,
        &mut out.sink(),
    );
    let mut audio = Vec::new();
    let mut video = Vec::new();
    for e in out.events() {
        match e {
            Event::Audio { pts90, .. } => audio.push(pts90.unwrap()),
            Event::Video { pts90, .. } => video.push(pts90.unwrap()),
            Event::Pes {
                video: false, es, ..
            } => {
                // ADTS header, then the frame.
                assert_eq!(&es[..2], &[0xFF, 0xF9]);
                assert_eq!(&es[7..], AAC_SILENCE);
            }
            _ => {}
        }
    }
    assert_eq!(audio.len(), 2);
    assert_eq!(audio[0], TsClock::ticks(audio_capture));
    assert_eq!(
        audio[0] - video[0],
        5 * 90,
        "A/V offset is the capture offset"
    );
    assert_eq!(m.late_audio_frames(), 1, "95 ms after capture missed 90 ms");
    let audio_packets = out.packets(AUDIO_PID);
    assert_eq!(audio_packets[0][5] & 0x50, 0x40, "random access, no PCR");
}

/// The bytes of one Opus access unit on the wire, as the Swift muxer wrote
/// them: PMT entry, PES on stream_id 0xBD with the PTS, control header,
/// packet.
#[test]
fn opus_pes_bytes() {
    let mut m = h264_muxer(Some(AudioCodec::Opus), 80 * MS);
    m.set_audio_format(48_000, 1).unwrap();
    let mut out = Out::default();
    let start = 7_000 * MS;
    let key = slice(true);
    m.write_video(
        &VideoFrame {
            data: &key,
            keyframe: true,
            pts_ns: start,
            sei: None,
        },
        start + 30 * MS,
        &mut out.sink(),
    );
    let opus = [0x78, 0x01, 0x02, 0x03, 0x04];
    let first_sample = start + 12 * MS;
    // AudioTiming.ptsNs: first sample minus the 312-sample pre-skip.
    let pts_ns = first_sample - 312 * 1_000_000_000 / 48_000;
    m.write_audio(&opus, pts_ns, first_sample + 15 * MS, &mut out.sink());

    let pmt = out.packets(PMT_PID)[0];
    let entry = [
        0x06, 0xE1, 0x01, 0xF0, 0x0A, 0x05, 0x04, 0x4F, 0x70, 0x75, 0x73, 0x7F, 0x02, 0x80, 0x01,
    ];
    assert!(
        pmt.windows(entry.len()).any(|w| w == entry),
        "Opus PMT entry"
    );
    let audio = out.packets(AUDIO_PID);
    assert_eq!(audio.len(), 1, "a 10 ms Opus frame fits one TS packet");
    let packet = audio[0];
    assert_eq!(packet[1] & 0x40, 0x40, "payload unit start");
    let pes_len = 9 + 5 + 3 + opus.len();
    let pes = &packet[PACKET_SIZE - pes_len..];
    assert_eq!(
        &pes[..9],
        &[
            0x00,
            0x00,
            0x01,
            0xBD,
            0x00,
            (pes_len - 6) as u8,
            0x84,
            0x80,
            0x05
        ]
    );
    assert_eq!(
        probe_ts::pes_timestamps(pes).0,
        Some(TsClock::ticks(first_sample - 6_500_000))
    );
    assert_eq!(pes[9] & 0xF1, 0x21, "PTS-only marker bits");
    assert_eq!(&pes[14..17], &[0x7F, 0xE0, 0x05]);
    assert_eq!(&pes[17..], &opus);
    assert_eq!(m.late_audio_frames(), 0);
    let events = out.events();
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Pes { video: false, es, .. }
        if probe_ts::opus_packets(es) == Some(vec![&opus[..]]))));
}

/// Two seconds of 30 fps video with a keyframe every second and Opus every
/// 10 ms, frames of all sizes: tables before every keyframe, unbroken
/// continuity counters, the PCR on every video PES, every SEI read back.
#[test]
fn stream_reads_back_with_the_probe() {
    for codec in [VideoCodec::H264, VideoCodec::Hevc] {
        let mut m = Muxer::new(codec, Some(AudioCodec::Opus), 80 * MS);
        let (key, p): (&[u8], &[u8]) = match codec {
            VideoCodec::H264 => {
                m.set_video_format([SPS, PPS], 4).unwrap();
                (&[0x65, 0x88], &[0x41, 0x9A])
            }
            VideoCodec::Hevc => {
                m.set_video_format(
                    [
                        &[0x40, 0x01, 0x0C][..],
                        &[0x42, 0x01, 0x01][..],
                        &[0x44, 0x01, 0xC1][..],
                    ],
                    4,
                )
                .unwrap();
                (&[0x26, 0x01, 0xAF], &[0x02, 0x01, 0xD0])
            }
        };
        m.set_audio_format(48_000, 2).unwrap();
        let mut out = Out::default();
        let t0 = 3_600_000 * MS;
        let base_tai = 1_800_000_000_000_000_000u64;
        let mut audio_t = t0;
        for i in 0..60i64 {
            let t = t0 + i * 33_333_333;
            let keyframe = i % 30 == 0;
            let mut nal = if keyframe { key.to_vec() } else { p.to_vec() };
            // Sizes around the packet boundaries and large frames.
            nal.resize(nal.len() + (i as usize * 997) % 9000 + i as usize, 0x5A);
            let data = length_prefixed(&[&nal]);
            let frame = VideoFrame {
                data: &data,
                keyframe,
                pts_ns: t,
                sei: Some(fields(base_tai + i as u64 * 33_333_333, i as u32)),
            };
            assert!(m.write_video(&frame, t + 40 * MS, &mut out.sink()));
            while audio_t < t + 33 * MS {
                m.write_audio(&[0xFC; 40], audio_t, audio_t + 20 * MS, &mut out.sink());
                audio_t += 10 * MS;
            }
        }
        let mut frames = 0;
        let mut audio = 0;
        for e in out.events() {
            match e {
                Event::Video {
                    pts90,
                    sei,
                    keyframe,
                    pcr90,
                    ..
                } => {
                    let sei = sei.expect("SEI before the first slice");
                    assert_eq!(sei.frame_counter, frames);
                    assert_eq!(sei.capture_ns, base_tai + u64::from(frames) * 33_333_333);
                    assert_eq!(keyframe, frames % 30 == 0, "{codec:?} {frames}");
                    let t = t0 + i64::from(frames) * 33_333_333;
                    assert_eq!(pts90, Some(TsClock::ticks(t)));
                    assert_eq!(pcr90, Some(TsClock::ticks(t + 40 * MS - 80 * MS)));
                    frames += 1;
                }
                Event::Audio { .. } => audio += 1,
                Event::Discontinuity { pid } => panic!("CC jump on {pid}"),
                _ => {}
            }
        }
        assert_eq!(frames, 60);
        assert_eq!(audio, 200);
        // PAT and PMT once per keyframe, right before it.
        let tables: Vec<usize> = out
            .chunks
            .iter()
            .enumerate()
            .filter(|(_, c)| c.0 == Content::Tables)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(tables.len(), 2);
        for i in tables {
            assert_eq!(out.chunks[i + 1].0, Content::Video);
        }
        // Continuity counters of the tables too.
        for pid in [0, PMT_PID] {
            let ccs: Vec<u8> = out.packets(pid).iter().map(|p| p[3] & 0x0F).collect();
            assert_eq!(ccs, [0, 1]);
        }
        assert_eq!(m.late_video_frames(), 0);
        assert_eq!(m.late_audio_frames(), 0);
        assert!(out
            .packets(VIDEO_PID)
            .iter()
            .filter(|p| p[1] & 0x40 != 0)
            .all(|p| p[3] & 0x20 != 0 && p[5] & 0x10 != 0));
    }
}

/// Access units of an Annex B stream: a new one at every AUD.
fn access_units(stream: &[u8]) -> Vec<Vec<&[u8]>> {
    let mut units: Vec<Vec<&[u8]>> = Vec::new();
    for nal in annexb::split(stream) {
        if nal[0] & 0x1F == 9 || units.is_empty() {
            units.push(Vec::new());
        }
        units.last_mut().unwrap().push(nal);
    }
    units
}

fn tool(name: &str, version_flag: &str) -> bool {
    std::process::Command::new(name)
        .arg(version_flag)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Real x264 output (the Swift tests' sample) with AAC silence: ffprobe and
/// GStreamer find both streams, decode the video, and read the PTS as sent.
/// Writes `ts_mux_h264_aac.ts` to the test temp directory.
#[test]
fn real_h264_with_ffprobe_and_gstreamer() {
    let sample = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ios/TallyCamTests/Resources/sample.h264"
    ))
    .unwrap();
    let units = access_units(&sample);
    assert_eq!(units.len(), 10);
    let sps = units[0].iter().find(|n| n[0] & 0x1F == 7).unwrap();
    let pps = units[0].iter().find(|n| n[0] & 0x1F == 8).unwrap();
    let mut m = Muxer::new(
        VideoCodec::H264,
        Some(AudioCodec::Aac),
        AudioCodec::pcr_delay_ns(Some(AudioCodec::Aac)),
    );
    m.set_video_format([*sps, *pps], 4).unwrap();
    m.set_audio_format(48_000, 1).unwrap();
    let mut out = Out::default();
    let t0 = 10_000 * MS;
    let mut audio_t = t0;
    for (i, unit) in units.iter().enumerate() {
        let t = t0 + i as i64 * 40 * MS;
        let keyframe = unit.iter().any(|n| n[0] & 0x1F == 5);
        let data = length_prefixed(unit);
        m.write_video(
            &VideoFrame {
                data: &data,
                keyframe,
                pts_ns: t,
                sei: Some(fields(
                    1_760_361_637_000_000_000 + i as u64 * 40_000_000,
                    i as u32,
                )),
            },
            t + 30 * MS,
            &mut out.sink(),
        );
        while audio_t < t + 40 * MS {
            m.write_audio(AAC_SILENCE, audio_t, audio_t + 60 * MS, &mut out.sink());
            audio_t += 21_333_333;
        }
    }
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("ts_mux_h264_aac.ts");
    std::fs::write(&path, &out.bytes).unwrap();
    println!("muxed: {}", path.display());

    if tool("ffprobe", "-version") {
        let o = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_streams",
                "-show_packets",
                "-of",
                "compact",
            ])
            .arg(&path)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&o.stdout);
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        assert!(
            o.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(text.contains("codec_name=h264"), "{text}");
        assert!(text.contains("codec_name=aac"), "{text}");
        let video: Vec<&str> = text
            .lines()
            .filter(|l| l.starts_with("packet|codec_type=video"))
            .collect();
        assert_eq!(video.len(), 10);
        // pts as sent: ticks of the capture time.
        assert!(video[0].contains(&format!("|pts={}|", TsClock::ticks(t0))));
        assert!(video[0].contains("flags=K"));
    } else {
        println!("ffprobe not installed: skipped");
    }
    if tool("gst-launch-1.0", "--version") {
        let o = std::process::Command::new("gst-launch-1.0")
            .args(["-q", "filesrc"])
            .arg(format!("location={}", path.display()))
            .args([
                "!",
                "tsdemux",
                "name=d",
                "d.",
                "!",
                "queue",
                "!",
                "h264parse",
                "!",
                "avdec_h264",
                "!",
                "fakesink",
                "d.",
                "!",
                "queue",
                "!",
                "aacparse",
                "!",
                "avdec_aac",
                "!",
                "fakesink",
            ])
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
    } else {
        println!("gst-launch-1.0 not installed: skipped");
    }
}

/// The stream starts on a keyframe muxed before its PCR deadline: a late
/// first keyframe (the encoder warming up) is passed over, the muxer asks for
/// another, and the next keyframe in time starts the stream.
#[test]
fn a_late_first_keyframe_does_not_start_the_stream() {
    let mut m = h264_muxer(None, 60 * MS);
    let mut out = Out::default();
    let key = slice(true);
    let frame = |pts_ns| VideoFrame {
        data: &key,
        keyframe: true,
        pts_ns,
        sei: None,
    };
    let t = 1_000 * MS;
    // Muxed 61 ms after capture with a 60 ms delay: late.
    assert!(!m.write_video(&frame(t), t + 61 * MS, &mut out.sink()));
    assert!(m.wants_keyframe());
    assert_eq!((m.start_ns(), m.late_start_keyframes()), (None, 1));
    assert!(out.bytes.is_empty());
    // The next keyframe is in time.
    let t2 = t + 33 * MS;
    assert!(m.write_video(&frame(t2), t2 + 45 * MS, &mut out.sink()));
    assert_eq!(m.start_ns(), Some(t2));
    assert!(!m.wants_keyframe());
    assert_eq!(m.late_video_frames(), 0);
}

/// MediaCodec's layout (`nal_length_size` 0, the frames as Annex B) gives
/// byte for byte the stream VideoToolbox's (4-byte lengths) gives for the
/// same NAL units: the real x264 sample (its own AUD, SPS and PPS inline,
/// 3- and 4-byte start codes) and frames of all sizes in both codecs.
#[test]
fn annexb_frames_mux_like_length_prefixed_frames() {
    let sample = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ios/TallyCamTests/Resources/sample.h264"
    ))
    .unwrap();
    let units = access_units(&sample);
    let sps = units[0].iter().find(|n| n[0] & 0x1F == 7).unwrap();
    let pps = units[0].iter().find(|n| n[0] & 0x1F == 8).unwrap();
    let mut h264: Vec<(Vec<Vec<u8>>, bool)> = units
        .iter()
        .map(|u| {
            let keyframe = u.iter().any(|n| n[0] & 0x1F == 5);
            (u.iter().map(|n| n.to_vec()).collect(), keyframe)
        })
        .collect();
    let mut hevc = Vec::new();
    for i in 0..60usize {
        let keyframe = i % 30 == 0;
        let mut nal = if keyframe {
            vec![0x26, 0x01, 0xAF]
        } else {
            vec![0x02, 0x01, 0xD0]
        };
        nal.resize(nal.len() + (i * 997) % 9000 + i, 0x5A);
        // An encoder SEI first, as MediaCodec may write one.
        hevc.push((vec![vec![0x4E, 0x01, 0x05, 0x01, 0x80], nal], keyframe));
    }
    for i in 0..60usize {
        let mut nal = vec![if i % 30 == 0 { 0x65 } else { 0x41 }, 0x88];
        nal.resize(nal.len() + (i * 1499) % 20000, 0x33);
        h264.push((vec![nal], i % 30 == 0));
    }
    let hevc_sets: [&[u8]; 3] = [
        &[0x40, 0x01, 0x0C],
        &[0x42, 0x01, 0x01],
        &[0x44, 0x01, 0xC1],
    ];
    for (codec, frames, sets) in [
        (VideoCodec::H264, &h264, vec![&sps[..], &pps[..]]),
        (VideoCodec::Hevc, &hevc, hevc_sets.to_vec()),
    ] {
        let run = |nal_length_size: usize| {
            let mut m = Muxer::new(codec, Some(AudioCodec::Opus), 80 * MS);
            m.set_video_format(sets.iter().copied(), nal_length_size)
                .unwrap();
            m.set_audio_format(48_000, 1).unwrap();
            let mut out = Out::default();
            let t0 = 5_000 * MS;
            for (i, (nals, keyframe)) in frames.iter().enumerate() {
                let nals: Vec<&[u8]> = nals.iter().map(|n| &n[..]).collect();
                let data = if nal_length_size == 0 {
                    // Alternating 4- and 3-byte start codes.
                    let mut d = Vec::new();
                    for (j, n) in nals.iter().enumerate() {
                        d.extend_from_slice(&[0, 0, 0, 1][j % 2..]);
                        d.extend_from_slice(n);
                    }
                    d
                } else {
                    length_prefixed(&nals)
                };
                let t = t0 + i as i64 * 33 * MS;
                let frame = VideoFrame {
                    data: &data,
                    keyframe: *keyframe,
                    pts_ns: t,
                    sei: Some(fields(1_800_000_000_000_000_000 + i as u64, i as u32)),
                };
                assert!(m.write_video(&frame, t + 30 * MS, &mut out.sink()));
                m.write_audio(&[0xFC; 40], t, t + 20 * MS, &mut out.sink());
            }
            out
        };
        let expected = run(4);
        let annexb = run(annexb::ANNEX_B);
        assert_eq!(annexb.chunks, expected.chunks, "{codec:?}");
        assert!(annexb.bytes == expected.bytes, "{codec:?}: streams differ");
        let frames_read = annexb
            .events()
            .iter()
            .filter(|e| matches!(e, Event::Video { sei: Some(_), .. }))
            .count();
        assert_eq!(frames_read, frames.len());
    }
}

#[test]
fn nal_length_size_above_4_is_rejected() {
    let mut m = Muxer::new(VideoCodec::H264, None, 80 * MS);
    assert!(m.set_video_format([SPS, PPS], 5).is_err());
    for n in 0..=4 {
        m.set_video_format([SPS, PPS], n).unwrap();
    }
}
