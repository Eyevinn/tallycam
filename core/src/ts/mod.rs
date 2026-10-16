//! MPEG-TS muxer for the camera: H.264 or HEVC video and Opus or AAC audio
//! in one program, sans-IO.
//!
//! - [`Muxer`]: takes encoded access units and audio frames with their host
//!   times and hands out whole TS packets through a callback, one call per
//!   PES (and one for PAT/PMT before every keyframe). Its buffers are
//!   reused, so after the first frames nothing is allocated per frame.
//! - Timing ([`TsClock`]): PTS is the capture time on the host clock; the PCR
//!   is the time of muxing minus a fixed delay sized to the slowest stream
//!   (docs/LATENCY.md).
//! - Video arrives as the encoder emits it (length-prefixed NAL units, as
//!   from VideoToolbox) and leaves as Annex B with an access unit delimiter,
//!   the parameter sets on keyframes and the LCP timestamp SEI ([`crate::sei`])
//!   right before the first slice.
//! - Opus is carried as GStreamer and FFmpeg carry it (stream_type 0x06,
//!   registration 'Opus', control header per access unit); AAC as ADTS.

pub mod annexb;
pub mod audio;
mod clock;
pub mod packet;
pub mod psi;

pub use clock::TsClock;

use crate::sei;

pub const PACKET_SIZE: usize = 188;
pub const PAT_PID: u16 = 0;
pub const PMT_PID: u16 = 4095;
pub const VIDEO_PID: u16 = 256;
pub const AUDIO_PID: u16 = 257;
pub const PROGRAM_NUMBER: u16 = 1;
const VIDEO_STREAM_ID: u8 = 0xE0;
const AAC_STREAM_ID: u8 = 0xC0;

/// PCR delay with AAC audio: covers audio's capture-to-mux time with a
/// margin. The AAC PTS are moved back by the 2112 samples of priming (44 ms),
/// and a frame is muxed once its 1024 samples (21 ms) are in; see
/// docs/LATENCY.md for the measurements.
pub const AAC_PCR_DELAY_NS: i64 = 150_000_000;
/// PCR delay without audio, and with Opus (audio is then faster than video):
/// covers video's capture-to-mux time (camera delivery and encoding, p95
/// 50 ms, max 71 ms measured).
pub const VIDEO_PCR_DELAY_NS: i64 = 80_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
}

impl VideoCodec {
    /// stream_type in the PMT.
    pub fn stream_type(self) -> u8 {
        match self {
            VideoCodec::H264 => 0x1B,
            VideoCodec::Hevc => 0x24,
        }
    }

    fn sei_codec(self) -> sei::Codec {
        match self {
            VideoCodec::H264 => sei::Codec::H264,
            VideoCodec::Hevc => sei::Codec::Hevc,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    /// One Opus packet per frame (10 ms at 48 kHz).
    Opus,
    /// One raw AAC-LC frame (no ADTS header) per frame.
    Aac,
}

impl AudioCodec {
    /// The PCR delay for a stream with this audio (`None`: video only).
    pub fn pcr_delay_ns(audio: Option<AudioCodec>) -> i64 {
        match audio {
            Some(AudioCodec::Aac) => AAC_PCR_DELAY_NS,
            Some(AudioCodec::Opus) | None => VIDEO_PCR_DELAY_NS,
        }
    }
}

/// What a chunk of TS packets carries: the sender times video frames and
/// may prioritise audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Content {
    /// PAT and PMT.
    Tables,
    Video,
    Audio,
}

/// One encoded video frame.
#[derive(Debug, Clone, Copy)]
pub struct VideoFrame<'a> {
    /// NAL units, each preceded by a big-endian length of the size given to
    /// [`Muxer::set_video_format`] (VideoToolbox's AVCC/HVCC layout).
    pub data: &'a [u8],
    pub keyframe: bool,
    /// Capture time on the host clock, ns: the PTS.
    pub pts_ns: i64,
    /// The LCP timestamp SEI to put before the first slice.
    pub sei: Option<sei::Fields>,
}

#[derive(Debug)]
pub enum FormatError {
    NalLengthSize(usize),
    /// A parameter set the codec needs is missing (its NAL type).
    MissingParameterSet(u8),
    /// AAC: no ADTS sampling frequency index for the rate.
    AacSampleRate(u32),
    /// Opus: only 48 kHz, 1 or 2 channels.
    OpusFormat {
        sample_rate: u32,
        channels: u8,
    },
}

impl std::fmt::Display for FormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatError::NalLengthSize(n) => write!(f, "NAL length size {n} is not 1 to 4"),
            FormatError::MissingParameterSet(t) => {
                write!(f, "parameter set of NAL type {t} missing")
            }
            FormatError::AacSampleRate(r) => {
                write!(f, "AAC sample rate {r} has no ADTS frequency index")
            }
            FormatError::OpusFormat {
                sample_rate,
                channels,
            } => write!(
                f,
                "Opus at {sample_rate} Hz, {channels} ch is not supported"
            ),
        }
    }
}

impl std::error::Error for FormatError {}

/// Muxes one program. Not thread safe; call from one thread (or queue).
pub struct Muxer {
    video: VideoCodec,
    audio: Option<AudioCodec>,
    clock: TsClock,
    /// The parameter sets in Annex B (start codes included), written after
    /// the AUD of every keyframe; empty until the video format is known.
    parameter_sets: Vec<u8>,
    nal_length_size: usize,
    /// AAC only.
    aac_frequency_index: u8,
    audio_channels: u8,
    audio_ready: bool,
    /// Capture time of the first video frame (a keyframe).
    start_ns: Option<i64>,
    /// PAT and PMT, continuity counters patched in at each write.
    tables: [u8; 2 * PACKET_SIZE],
    cc_pat: u8,
    cc_pmt: u8,
    cc_video: u8,
    cc_audio: u8,
    pes: Vec<u8>,
    ts: Vec<u8>,
    late_video_frames: u64,
    late_audio_frames: u64,
    /// Keyframes not used to start the stream because they were late.
    late_start_keyframes: u64,
}

impl Muxer {
    /// `audio`: `None` for video only. `pcr_delay_ns`: see [`TsClock`];
    /// [`AudioCodec::pcr_delay_ns`] gives the default.
    pub fn new(video: VideoCodec, audio: Option<AudioCodec>, pcr_delay_ns: i64) -> Self {
        let mut m = Muxer {
            video,
            audio,
            clock: TsClock::new(pcr_delay_ns),
            parameter_sets: Vec::with_capacity(512),
            nal_length_size: 4,
            aac_frequency_index: 0,
            audio_channels: 1,
            audio_ready: false,
            start_ns: None,
            tables: [0; 2 * PACKET_SIZE],
            cc_pat: 0,
            cc_pmt: 0,
            cc_video: 0,
            cc_audio: 0,
            // Audio PES only (video is written straight into `ts`).
            pes: Vec::with_capacity(4096),
            // A 1080p keyframe is a few hundred kB; the buffer grows to the
            // largest frame once and is reused.
            ts: Vec::with_capacity(256 * 1024 / 184 * PACKET_SIZE + 2 * PACKET_SIZE),
            late_video_frames: 0,
            late_audio_frames: 0,
            late_start_keyframes: 0,
        };
        m.build_tables();
        m
    }

    pub fn video_codec(&self) -> VideoCodec {
        self.video
    }

    pub fn audio_codec(&self) -> Option<AudioCodec> {
        self.audio
    }

    pub fn clock(&self) -> TsClock {
        self.clock
    }

    /// Video samples muxed after their PCR deadline (`PTS < PCR`): a
    /// receiver gets them late. Should stay 0; if not, raise the PCR delay.
    pub fn late_video_frames(&self) -> u64 {
        self.late_video_frames
    }

    pub fn late_audio_frames(&self) -> u64 {
        self.late_audio_frames
    }

    /// True before the stream started when a keyframe came too late to start
    /// it: the encoder should make the next frame a keyframe.
    pub fn wants_keyframe(&self) -> bool {
        self.start_ns.is_none() && self.late_start_keyframes > 0
    }

    /// Keyframes passed over at the start because they were late.
    pub fn late_start_keyframes(&self) -> u64 {
        self.late_start_keyframes
    }

    /// Host ns of the first muxed video frame, once the stream started.
    pub fn start_ns(&self) -> Option<i64> {
        self.start_ns
    }

    /// The codec's parameter sets (H.264: SPS and PPS; HEVC: VPS, SPS, PPS
    /// and any other the format has), in order, without start codes, and
    /// the length prefix size of the frames. On error the previous format is
    /// dropped and video frames are not muxed until a valid one arrives.
    pub fn set_video_format<'a>(
        &mut self,
        sets: impl IntoIterator<Item = &'a [u8]>,
        nal_length_size: usize,
    ) -> Result<(), FormatError> {
        self.parameter_sets.clear();
        if !(1..=4).contains(&nal_length_size) {
            return Err(FormatError::NalLengthSize(nal_length_size));
        }
        let mut seen = [false; 3];
        for set in sets {
            if set.is_empty() {
                continue;
            }
            let t = annexb::nal_type(self.video, set[0]);
            let required: &[u8] = match self.video {
                VideoCodec::H264 => &[annexb::H264_SPS, annexb::H264_PPS],
                VideoCodec::Hevc => &[annexb::HEVC_VPS, annexb::HEVC_SPS, annexb::HEVC_PPS],
            };
            if let Some(i) = required.iter().position(|&r| r == t) {
                seen[i] = true;
            }
            self.parameter_sets.extend_from_slice(&annexb::START_CODE);
            self.parameter_sets.extend_from_slice(set);
        }
        let missing = match self.video {
            VideoCodec::H264 => [annexb::H264_SPS, annexb::H264_PPS]
                .iter()
                .zip(seen)
                .find(|(_, s)| !s)
                .map(|(t, _)| *t),
            VideoCodec::Hevc => [annexb::HEVC_VPS, annexb::HEVC_SPS, annexb::HEVC_PPS]
                .iter()
                .zip(seen)
                .find(|(_, s)| !s)
                .map(|(t, _)| *t),
        };
        if let Some(t) = missing {
            self.parameter_sets.clear();
            return Err(FormatError::MissingParameterSet(t));
        }
        self.nal_length_size = nal_length_size;
        Ok(())
    }

    /// The audio encoder's output format. Audio is muxed (and the stream
    /// starts) only once it is valid for the codec.
    pub fn set_audio_format(&mut self, sample_rate: u32, channels: u8) -> Result<(), FormatError> {
        self.audio_channels = channels;
        let result = match self.audio {
            Some(AudioCodec::Aac) => match audio::adts_frequency_index(sample_rate) {
                Some(i) => {
                    self.aac_frequency_index = i;
                    Ok(())
                }
                None => Err(FormatError::AacSampleRate(sample_rate)),
            },
            Some(AudioCodec::Opus) => {
                if sample_rate == 48_000 && (1..=2).contains(&channels) {
                    Ok(())
                } else {
                    Err(FormatError::OpusFormat {
                        sample_rate,
                        channels,
                    })
                }
            }
            None => Ok(()),
        };
        if self.audio.is_some() {
            self.audio_ready = result.is_ok();
        }
        self.build_tables();
        result
    }

    /// Muxes one video frame: PAT and PMT first on a keyframe, then the
    /// frame's PES with a PCR. Returns false when the frame was not muxed:
    /// no video format yet, or the stream has not started (it starts on a
    /// keyframe once the audio format is known as well).
    pub fn write_video(
        &mut self,
        frame: &VideoFrame,
        now_ns: i64,
        out: &mut impl FnMut(&[u8], Content),
    ) -> bool {
        if self.parameter_sets.is_empty() {
            return false;
        }
        if self.start_ns.is_none() {
            // The stream starts on a keyframe that is in time: the
            // encoder's first frames take longer (up to 35 ms more measured
            // on an iPhone), and a first keyframe muxed after its PCR
            // deadline reaches a syncing receiver late. The caller asks the
            // encoder for another keyframe ([`Self::wants_keyframe`]).
            if !frame.keyframe || (self.audio.is_some() && !self.audio_ready) {
                return false;
            }
            if self.clock.slack_ns(frame.pts_ns, now_ns) < 0 {
                self.late_start_keyframes += 1;
                return false;
            }
            self.start_ns = Some(frame.pts_ns);
        }
        let mut sei_buf = [0u8; sei::MAX_NAL_LEN];
        let sei_nal = frame.sei.map(|f| {
            let n = sei::write_nal_unit(&f, self.video.sei_codec(), &mut sei_buf);
            &sei_buf[..n]
        });
        if frame.keyframe {
            self.write_tables(out);
        }
        if self.clock.slack_ns(frame.pts_ns, now_ns) < 0 {
            self.late_video_frames += 1;
        }
        let pts = self.clock.pts(frame.pts_ns);
        let parameter_sets = frame.keyframe.then_some(&self.parameter_sets[..]);
        // Sized first (only the NAL lengths are read), then written straight
        // into TS packets: the frame is copied once, into `ts`.
        let mut size = annexb::ByteCount::default();
        annexb::append_access_unit(
            &mut size,
            self.video,
            frame.data,
            self.nal_length_size,
            parameter_sets,
            sei_nal,
        );
        self.ts.clear();
        let header = packet::pes_header(VIDEO_STREAM_ID, pts, size.0);
        let mut pes = packet::PesWriter::new(
            &mut self.ts,
            VIDEO_PID,
            frame.keyframe,
            Some(self.clock.pcr(now_ns)),
            &mut self.cc_video,
            header.len() + size.0,
        );
        pes.write(&header);
        annexb::append_access_unit(
            &mut pes,
            self.video,
            frame.data,
            self.nal_length_size,
            parameter_sets,
            sei_nal,
        );
        debug_assert_eq!(pes.left(), 0);
        out(&self.ts, Content::Video);
        true
    }

    /// Muxes one audio frame (`frame`: one Opus packet, or one raw AAC frame
    /// without ADTS header). `pts_ns`: capture time of its first sample on
    /// the host clock, encoder delay already taken off. Returns false when
    /// it was not muxed: no valid audio format, the stream has not started,
    /// or the frame is from before the first video frame.
    pub fn write_audio(
        &mut self,
        frame: &[u8],
        pts_ns: i64,
        now_ns: i64,
        out: &mut impl FnMut(&[u8], Content),
    ) -> bool {
        let (Some(start_ns), Some(codec)) = (self.start_ns, self.audio) else {
            return false;
        };
        if !self.audio_ready || pts_ns < start_ns {
            return false;
        }
        if self.clock.slack_ns(pts_ns, now_ns) < 0 {
            self.late_audio_frames += 1;
        }
        let pts = self.clock.pts(pts_ns);
        match codec {
            AudioCodec::Aac => {
                packet::begin_pes(&mut self.pes, AAC_STREAM_ID, pts);
                self.pes.extend_from_slice(&audio::adts_header(
                    self.aac_frequency_index,
                    self.audio_channels,
                    frame.len(),
                ));
            }
            AudioCodec::Opus => {
                packet::begin_pes(&mut self.pes, audio::OPUS_STREAM_ID, pts);
                audio::append_opus_control_header(&mut self.pes, frame.len());
            }
        }
        self.pes.extend_from_slice(frame);
        packet::finish_pes(&mut self.pes);
        self.ts.clear();
        packet::packetize(
            &self.pes,
            AUDIO_PID,
            true,
            None,
            &mut self.cc_audio,
            &mut self.ts,
        );
        out(&self.ts, Content::Audio);
        true
    }

    fn write_tables(&mut self, out: &mut impl FnMut(&[u8], Content)) {
        self.tables[3] = 0x10 | self.cc_pat;
        self.tables[PACKET_SIZE + 3] = 0x10 | self.cc_pmt;
        self.cc_pat = (self.cc_pat + 1) & 0x0F;
        self.cc_pmt = (self.cc_pmt + 1) & 0x0F;
        out(&self.tables, Content::Tables);
    }

    fn build_tables(&mut self) {
        let mut streams = [psi::Stream {
            stream_type: self.video.stream_type(),
            pid: VIDEO_PID,
            descriptors: &[],
        }; 2];
        let opus_descriptors = audio::opus_descriptors(self.audio_channels);
        let count = match self.audio {
            Some(AudioCodec::Aac) => {
                streams[1] = psi::Stream {
                    stream_type: 0x0F,
                    pid: AUDIO_PID,
                    descriptors: &[],
                };
                2
            }
            Some(AudioCodec::Opus) => {
                streams[1] = psi::Stream {
                    stream_type: 0x06,
                    pid: AUDIO_PID,
                    descriptors: &opus_descriptors,
                };
                2
            }
            None => 1,
        };
        let (pat, pmt) = self.tables.split_at_mut(PACKET_SIZE);
        psi::write_packet(pat, PAT_PID, |s| psi::pat_section(s, PMT_PID));
        psi::write_packet(pmt, PMT_PID, |s| {
            psi::pmt_section(s, VIDEO_PID, &streams[..count])
        });
    }
}
