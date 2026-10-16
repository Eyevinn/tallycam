//! A minimal MPEG-TS reader for arrival-time measurements: PAT, PMT, PCR,
//! PES timestamps, and the LCP capture timestamp SEI at the start of each
//! video access unit, parsed with the core.
//!
//! It reports a video frame as soon as its SEI (or its first slice) has
//! arrived, not when the whole access unit is complete, so the arrival time
//! is the time the frame's first bytes were handed to the application. With
//! SRT that is the same instant as the last bytes: every packet of a frame
//! is stamped at enqueue on the sender, and TSBPD releases them together.

use tallycam_core::sei::{self, Fields};

pub const PACKET: usize = 188;
const SYNC: u8 = 0x47;
/// Bytes of a video PES collected while looking for the SEI. Parameter sets
/// and the SEI are a few hundred bytes; stop looking after this.
const SEI_SEARCH_LIMIT: usize = 8192;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
}

/// The audio stream's codec, from the PMT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    /// ADTS AAC (stream_type 0x0F).
    Aac,
    /// Opus (stream_type 0x06 with registration 'Opus'), with its channel
    /// count from the extension descriptor (channel_config_code 1 or 2;
    /// other codes are read as 2).
    Opus { channels: u8 },
    /// Any other audio stream type (LATM, MPEG audio).
    Other(u8),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The first bytes of a video access unit, with its timestamps.
    Video {
        pts90: Option<u64>,
        dts90: Option<u64>,
        sei: Option<Fields>,
        keyframe: bool,
        /// The last PCR seen before this frame, 90 kHz.
        pcr90: Option<u64>,
    },
    /// The start of an audio PES.
    Audio {
        pts90: Option<u64>,
        pcr90: Option<u64>,
    },
    /// A PCR.
    Pcr { pcr90: u64 },
    /// A continuity counter jump on a media PID: TS packets were lost.
    Discontinuity { pid: u16 },
    /// A complete PES (only with `collect_pes`): its elementary stream data,
    /// for decoding with the PTS exactly as sent.
    Pes {
        video: bool,
        pts90: Option<u64>,
        es: Vec<u8>,
    },
}

/// A PES being collected.
#[derive(Default)]
struct PesBuf {
    data: Vec<u8>,
}

impl PesBuf {
    /// Total PES size from its header, when the length field is set.
    fn expected(&self) -> Option<usize> {
        let len = (usize::from(*self.data.get(4)?) << 8) | usize::from(*self.data.get(5)?);
        (len != 0).then_some(6 + len)
    }

    fn take(&mut self, video: bool) -> Option<Event> {
        let data = std::mem::take(&mut self.data);
        if data.len() < 9 {
            return None;
        }
        let header_len = 9 + data[8] as usize;
        let (pts90, _) = pes_timestamps(&data);
        Some(Event::Pes {
            video,
            pts90,
            es: data.get(header_len..)?.to_vec(),
        })
    }
}

#[derive(Default)]
struct VideoPes {
    active: bool,
    header_done: bool,
    header: Vec<u8>,
    es: Vec<u8>,
    pts90: Option<u64>,
    dts90: Option<u64>,
    reported: bool,
}

#[derive(Default)]
pub struct TsReader {
    rest: Vec<u8>,
    pmt_pid: Option<u16>,
    video_pid: Option<u16>,
    video_codec: Option<VideoCodec>,
    audio_pid: Option<u16>,
    audio_codec: Option<AudioCodec>,
    pcr_pid: Option<u16>,
    last_pcr90: Option<u64>,
    video: VideoPes,
    cc: Vec<Option<u8>>,
    /// Emit complete PES packets as well (`Event::Pes`).
    pub collect_pes: bool,
    pes_video: PesBuf,
    pes_audio: PesBuf,
    pub packets: u64,
}

impl TsReader {
    pub fn new() -> Self {
        TsReader {
            cc: vec![None; 8192],
            ..Default::default()
        }
    }

    /// Feeds bytes in any chunking; calls `on` for each event.
    pub fn feed(&mut self, mut data: &[u8], on: &mut impl FnMut(Event)) {
        if !self.rest.is_empty() {
            let need = PACKET - self.rest.len();
            let take = need.min(data.len());
            self.rest.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.rest.len() < PACKET {
                return;
            }
            let packet: [u8; PACKET] = self.rest[..].try_into().unwrap();
            self.rest.clear();
            self.packet(&packet, on);
        }
        while data.len() >= PACKET {
            if data[0] != SYNC {
                // Resynchronise on the next sync byte.
                data = &data[1..];
                continue;
            }
            let (packet, tail) = data.split_at(PACKET);
            self.packet(packet, on);
            data = tail;
        }
        self.rest.extend_from_slice(data);
    }

    fn packet(&mut self, p: &[u8], on: &mut impl FnMut(Event)) {
        self.packets += 1;
        let pusi = p[1] & 0x40 != 0;
        let pid = u16::from(p[1] & 0x1F) << 8 | u16::from(p[2]);
        let afc = (p[3] >> 4) & 0x3;
        let cc = p[3] & 0x0F;
        let mut i = 4;
        if afc & 0x2 != 0 {
            let len = p[4] as usize;
            if len > 0 && 5 + len <= PACKET {
                let flags = p[5];
                if flags & 0x10 != 0 && len >= 7 && Some(pid) == self.pcr_pid {
                    let b = &p[6..12];
                    let base = u64::from(b[0]) << 25
                        | u64::from(b[1]) << 17
                        | u64::from(b[2]) << 9
                        | u64::from(b[3]) << 1
                        | u64::from(b[4]) >> 7;
                    self.last_pcr90 = Some(base);
                    on(Event::Pcr { pcr90: base });
                }
            }
            i = 5 + len;
        }
        let has_payload = afc & 0x1 != 0;
        if has_payload && (Some(pid) == self.video_pid || Some(pid) == self.audio_pid) {
            if let Some(last) = self.cc[pid as usize] {
                if cc != (last + 1) & 0x0F && cc != last {
                    on(Event::Discontinuity { pid });
                }
            }
            self.cc[pid as usize] = Some(cc);
        }
        if !has_payload || i >= PACKET {
            return;
        }
        let payload = &p[i..];
        if self.collect_pes && (Some(pid) == self.video_pid || Some(pid) == self.audio_pid) {
            let video = Some(pid) == self.video_pid;
            let buf = if video {
                &mut self.pes_video
            } else {
                &mut self.pes_audio
            };
            if pusi {
                if let Some(e) = buf.take(video) {
                    on(e);
                }
            }
            if pusi || !buf.data.is_empty() {
                buf.data.extend_from_slice(payload);
            }
            if buf.expected().is_some_and(|n| buf.data.len() >= n) {
                let n = buf.expected().unwrap_or(0);
                buf.data.truncate(n);
                if let Some(e) = buf.take(video) {
                    on(e);
                }
            }
        }
        if pid == 0 {
            if pusi {
                self.pat(payload);
            }
        } else if Some(pid) == self.pmt_pid {
            if pusi {
                self.pmt(payload);
            }
        } else if Some(pid) == self.video_pid {
            self.video_payload(payload, pusi, on);
        } else if Some(pid) == self.audio_pid && pusi {
            let (pts90, _) = pes_timestamps(payload);
            on(Event::Audio {
                pts90,
                pcr90: self.last_pcr90,
            });
        }
    }

    /// The audio codec, once a PMT has been seen.
    pub fn audio_codec(&self) -> Option<AudioCodec> {
        self.audio_codec
    }

    /// The video codec, once a PMT has been seen.
    pub fn video_codec(&self) -> Option<VideoCodec> {
        self.video_codec
    }

    fn pat(&mut self, payload: &[u8]) {
        let Some(section) = section(payload) else {
            return;
        };
        // After the 8-byte header: program_number(2) pid(2) ..., then CRC.
        let mut j = 8;
        while j + 4 <= section.len().saturating_sub(4) {
            let program = u16::from(section[j]) << 8 | u16::from(section[j + 1]);
            let pid = u16::from(section[j + 2] & 0x1F) << 8 | u16::from(section[j + 3]);
            if program != 0 {
                self.pmt_pid = Some(pid);
                return;
            }
            j += 4;
        }
    }

    fn pmt(&mut self, payload: &[u8]) {
        let Some(s) = section(payload) else { return };
        if s.len() < 12 {
            return;
        }
        self.pcr_pid = Some(u16::from(s[8] & 0x1F) << 8 | u16::from(s[9]));
        let info_len = (usize::from(s[10] & 0x0F) << 8) | usize::from(s[11]);
        let mut j = 12 + info_len;
        let end = s.len().saturating_sub(4);
        while j + 5 <= end {
            let stream_type = s[j];
            let pid = u16::from(s[j + 1] & 0x1F) << 8 | u16::from(s[j + 2]);
            let es_len = (usize::from(s[j + 3] & 0x0F) << 8) | usize::from(s[j + 4]);
            match stream_type {
                0x1B => {
                    self.video_pid = Some(pid);
                    self.video_codec = Some(VideoCodec::H264);
                }
                0x24 => {
                    self.video_pid = Some(pid);
                    self.video_codec = Some(VideoCodec::Hevc);
                }
                0x0F | 0x11 | 0x03 | 0x04 if self.audio_pid.is_none() => {
                    self.audio_pid = Some(pid);
                    self.audio_codec = Some(if stream_type == 0x0F {
                        AudioCodec::Aac
                    } else {
                        AudioCodec::Other(stream_type)
                    });
                }
                // Private data: audio only when registered as Opus.
                0x06 if self.audio_pid.is_none() => {
                    let end_es = (j + 5 + es_len).min(end);
                    if let Some(channels) = opus_channels(&s[j + 5..end_es]) {
                        self.audio_pid = Some(pid);
                        self.audio_codec = Some(AudioCodec::Opus { channels });
                    }
                }
                _ => {}
            }
            j += 5 + es_len;
        }
    }

    fn video_payload(&mut self, payload: &[u8], pusi: bool, on: &mut impl FnMut(Event)) {
        if pusi {
            self.video = VideoPes {
                active: true,
                ..Default::default()
            };
        }
        let v = &mut self.video;
        if !v.active || v.reported {
            return;
        }
        let mut payload = payload;
        if !v.header_done {
            v.header.extend_from_slice(payload);
            if v.header.len() < 9 {
                return;
            }
            let header_len = 9 + v.header[8] as usize;
            if v.header.len() < header_len {
                return;
            }
            let (pts, dts) = pes_timestamps(&v.header);
            v.pts90 = pts;
            v.dts90 = dts;
            v.header_done = true;
            v.es.extend_from_slice(&v.header[header_len..]);
            v.header.clear();
            payload = &[];
        }
        v.es.extend_from_slice(payload);
        let codec = self.video_codec.unwrap_or(VideoCodec::H264);
        if let Some((sei, keyframe)) = scan_access_unit(&v.es, codec, v.es.len() > SEI_SEARCH_LIMIT)
        {
            v.reported = true;
            on(Event::Video {
                pts90: v.pts90,
                dts90: v.dts90,
                sei,
                keyframe,
                pcr90: self.last_pcr90,
            });
            v.es = Vec::new();
        }
    }
}

/// The section of a PSI payload (after the pointer field), up to its length.
/// The channel count of an Opus ES (registration descriptor 'Opus'), from
/// its extension descriptor (tag 0x7F, extension tag 0x80); 2 if absent.
fn opus_channels(descriptors: &[u8]) -> Option<u8> {
    let mut registered = false;
    let mut channels = 2;
    let mut i = 0;
    while i + 2 <= descriptors.len() {
        let (tag, len) = (descriptors[i], descriptors[i + 1] as usize);
        let body = descriptors.get(i + 2..i + 2 + len)?;
        match tag {
            0x05 if body.starts_with(b"Opus") => registered = true,
            0x7F if body.len() >= 2 && body[0] == 0x80 => {
                channels = if body[1] == 1 { 1 } else { 2 };
            }
            _ => {}
        }
        i += 2 + len;
    }
    registered.then_some(channels)
}

/// Splits the payload of an Opus PES into Opus packets: each is prefixed
/// with the control header (11-bit prefix 0x3FF, trim and extension flags,
/// `au_size` as 0xFF bytes plus a last byte below 255, then the optional
/// trim and extension fields). Returns None on a malformed header.
pub fn opus_packets(mut es: &[u8]) -> Option<Vec<&[u8]>> {
    let mut packets = Vec::new();
    while !es.is_empty() {
        if es.len() < 2 || es[0] != 0x7F || es[1] & 0xE0 != 0xE0 {
            return None;
        }
        let flags = es[1];
        let mut i = 2;
        let mut size = 0usize;
        loop {
            let b = *es.get(i)?;
            i += 1;
            size += b as usize;
            if b != 0xFF {
                break;
            }
        }
        if flags & 0x10 != 0 {
            i += 2; // start_trim
        }
        if flags & 0x08 != 0 {
            i += 2; // end_trim
        }
        if flags & 0x04 != 0 {
            i += 1 + *es.get(i)? as usize; // control extension
        }
        let packet = es.get(i..i + size)?;
        packets.push(packet);
        es = &es[i + size..];
    }
    Some(packets)
}

fn section(payload: &[u8]) -> Option<&[u8]> {
    let pointer = *payload.first()? as usize;
    let s = payload.get(1 + pointer..)?;
    if s.len() < 3 {
        return None;
    }
    let len = (usize::from(s[1] & 0x0F) << 8) | usize::from(s[2]);
    s.get(..3 + len)
}

fn read_timestamp(b: &[u8]) -> u64 {
    (u64::from(b[0] >> 1) & 0x07) << 30
        | u64::from(b[1]) << 22
        | (u64::from(b[2]) >> 1) << 15
        | u64::from(b[3]) << 7
        | u64::from(b[4]) >> 1
}

/// PTS and DTS of a PES packet starting at `p` (90 kHz).
pub fn pes_timestamps(p: &[u8]) -> (Option<u64>, Option<u64>) {
    if p.len() < 9 || p[0] != 0 || p[1] != 0 || p[2] != 1 {
        return (None, None);
    }
    let flags = p[7] >> 6;
    let pts = (flags & 0x2 != 0 && p.len() >= 14).then(|| read_timestamp(&p[9..14]));
    let dts = (flags == 0x3 && p.len() >= 19).then(|| read_timestamp(&p[14..19]));
    (pts, dts)
}

/// Looks through the start of an Annex B access unit. Returns once the first
/// slice has started: the LCP SEI if one came before it, and whether the
/// access unit is a keyframe. `give_up` returns what was found so far.
fn scan_access_unit(es: &[u8], codec: VideoCodec, give_up: bool) -> Option<(Option<Fields>, bool)> {
    let starts = start_codes(es);
    let mut sei = None;
    let mut keyframe = false;
    for (n, &(_, header)) in starts.iter().enumerate() {
        let Some(&first) = es.get(header) else { break };
        let (is_sei, is_slice, is_idr) = match codec {
            VideoCodec::H264 => {
                let t = first & 0x1F;
                (t == 6, (1..=5).contains(&t), t == 5 || t == 7)
            }
            VideoCodec::Hevc => {
                let t = (first >> 1) & 0x3F;
                (t == 39, t <= 31, (16..=21).contains(&t) || t == 32)
            }
        };
        keyframe |= is_idr;
        if is_slice {
            return Some((sei, keyframe));
        }
        if is_sei && sei.is_none() {
            // The NAL ends at the next start code; wait for it.
            let Some(&(next, _)) = starts.get(n + 1) else {
                break;
            };
            if let Some((_, fields)) = sei::parse_nal_unit(&es[header..next]) {
                sei = Some(fields);
            }
        }
    }
    give_up.then_some((sei, keyframe))
}

/// (start code position, NAL header position) for each start code.
fn start_codes(es: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 3 <= es.len() {
        if es[i] == 0 && es[i + 1] == 0 && es[i + 2] == 1 {
            let start = if i > 0 && es[i - 1] == 0 { i - 1 } else { i };
            out.push((start, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tallycam_core::sei::Codec;

    fn ts_timestamp(v: u64, marker: u8) -> [u8; 5] {
        [
            marker << 4 | ((v >> 29) & 0x0E) as u8 | 1,
            (v >> 22) as u8,
            ((v >> 14) & 0xFE) as u8 | 1,
            (v >> 7) as u8,
            ((v << 1) & 0xFE) as u8 | 1,
        ]
    }

    fn packet(pid: u16, pusi: bool, cc: u8, pcr: Option<u64>, payload: &[u8]) -> Vec<u8> {
        let mut p = vec![
            SYNC,
            (pid >> 8) as u8 | if pusi { 0x40 } else { 0 },
            pid as u8,
        ];
        // Adaptation field content (after its length byte).
        let mut af = Vec::new();
        if let Some(pcr) = pcr {
            af.extend_from_slice(&[
                0x10,
                (pcr >> 25) as u8,
                (pcr >> 17) as u8,
                (pcr >> 9) as u8,
                (pcr >> 1) as u8,
                ((pcr & 1) << 7) as u8 | 0x7E,
                0,
            ]);
        }
        let mut room = 184 - if af.is_empty() { 0 } else { 1 + af.len() };
        let body = &payload[..payload.len().min(room)];
        let mut stuffing = room - body.len();
        if af.is_empty() && stuffing > 0 {
            // A new adaptation field: its length byte takes one byte.
            stuffing -= 1;
            room -= 1;
            if stuffing > 0 {
                af.push(0);
                stuffing -= 1;
            }
        }
        let _ = room;
        let has_af = pcr.is_some() || body.len() < 184;
        p.push(if has_af { 0x30 } else { 0x10 } | cc);
        if has_af {
            af.extend(std::iter::repeat_n(0xFF, stuffing));
            p.push(af.len() as u8);
            p.extend_from_slice(&af);
        }
        p.extend_from_slice(body);
        assert_eq!(p.len(), PACKET);
        p
    }

    fn psi(table: &[u8]) -> Vec<u8> {
        let mut v = vec![0];
        v.extend_from_slice(table);
        v
    }

    fn pat() -> Vec<u8> {
        // table_id 0, length 13, tsid 1, version, sec 0/0, program 1 -> 4095, CRC (ignored).
        psi(&[
            0x00, 0xB0, 0x0D, 0, 1, 0xC1, 0, 0, 0, 1, 0xEF, 0xFF, 0, 0, 0, 0,
        ])
    }

    fn pmt() -> Vec<u8> {
        psi(&[
            0x02, 0xB0, 0x17, 0, 1, 0xC1, 0, 0, 0xE1, 0x00, 0xF0, 0x00, // PCR PID 256
            0x1B, 0xE1, 0x00, 0xF0, 0x00, // H.264 on 256
            0x0F, 0xE1, 0x01, 0xF0, 0x00, // AAC on 257
            0, 0, 0, 0,
        ])
    }

    #[test]
    fn reads_pcr_pts_and_sei_from_split_packets() {
        let fields = Fields {
            capture_ns: 1_800_000_000_123_456_789,
            synced: true,
            tai: true,
            frame_counter: 7,
            uncertainty_us: 400,
        };
        let mut es = vec![
            0, 0, 0, 1, 0x09, 0xF0, 0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE,
        ];
        es.extend_from_slice(&[0, 0, 0, 1]);
        es.extend_from_slice(&sei::nal_unit(&fields, Codec::H264));
        es.extend_from_slice(&[0, 0, 0, 1, 0x65]);
        es.extend(std::iter::repeat_n(0xAB, 400));
        let mut pes = vec![0, 0, 1, 0xE0, 0, 0, 0x84, 0x80, 5];
        pes.extend_from_slice(&ts_timestamp(123_456, 2));
        pes.extend_from_slice(&es);

        let mut stream = Vec::new();
        stream.extend(packet(0, true, 0, None, &pat()));
        stream.extend(packet(4095, true, 0, None, &pmt()));
        stream.extend(packet(256, true, 0, Some(120_000), &pes[..]));
        let first = PACKET - 4 - 8;
        stream.extend(packet(256, false, 1, None, &pes[first..]));
        let mut audio = vec![0, 0, 1, 0xC0, 0, 20, 0x84, 0x80, 5];
        audio.extend_from_slice(&ts_timestamp(122_000, 2));
        audio.extend_from_slice(&[0xFF; 10]);
        stream.extend(packet(257, true, 0, None, &audio));

        let mut reader = TsReader::new();
        let mut events = Vec::new();
        // Odd chunking, as from a network source.
        for chunk in stream.chunks(100) {
            reader.feed(chunk, &mut |e| events.push(e));
        }
        assert_eq!(
            events,
            vec![
                Event::Pcr { pcr90: 120_000 },
                Event::Video {
                    pts90: Some(123_456),
                    dts90: None,
                    sei: Some(fields),
                    keyframe: true,
                    pcr90: Some(120_000),
                },
                Event::Audio {
                    pts90: Some(122_000),
                    pcr90: Some(120_000),
                },
            ]
        );
        assert_eq!(reader.video_codec, Some(VideoCodec::H264));
    }

    #[test]
    fn reads_hevc_prefix_sei() {
        let fields = Fields {
            capture_ns: 1_800_000_000_987_654_321,
            synced: true,
            tai: true,
            frame_counter: 41,
            uncertainty_us: 700,
        };
        // The PMT TallyCam writes for HEVC (stream_type 0x24) and AAC.
        let pmt = psi(&[
            0x02, 0xB0, 0x17, 0, 1, 0xC1, 0, 0, 0xE1, 0x00, 0xF0, 0x00, // PCR PID 256
            0x24, 0xE1, 0x00, 0xF0, 0x00, // HEVC on 256
            0x0F, 0xE1, 0x01, 0xF0, 0x00, // AAC on 257
            0, 0, 0, 0,
        ]);
        let au = |keyframe: bool, counter: u32| {
            let f = Fields {
                frame_counter: counter,
                ..fields
            };
            // AUD (35), on keyframes VPS (32) SPS (33) PPS (34), an encoder
            // prefix SEI (39, another payload), ours, then the slice: IDR_W_RADL
            // (19) or TRAIL_R (1).
            let mut es = vec![0, 0, 0, 1, 0x46, 0x01, if keyframe { 0x10 } else { 0x30 }];
            if keyframe {
                es.extend_from_slice(&[0, 0, 0, 1, 0x40, 0x01, 0x0C, 0, 0, 0, 1, 0x42, 0x01, 0x01]);
                es.extend_from_slice(&[0, 0, 0, 1, 0x44, 0x01, 0xC1]);
            }
            es.extend_from_slice(&[0, 0, 0, 1, 0x4E, 0x01, 0x06, 0x01, 0xC4, 0x80]);
            es.extend_from_slice(&[0, 0, 0, 1]);
            es.extend_from_slice(&sei::nal_unit(&f, Codec::Hevc));
            es.extend_from_slice(&[0, 0, 0, 1, if keyframe { 0x26 } else { 0x02 }, 0x01]);
            es.extend(std::iter::repeat_n(0xAB, 300));
            let mut pes = vec![0, 0, 1, 0xE0, 0, 0, 0x84, 0x80, 5];
            pes.extend_from_slice(&ts_timestamp(90_000 + u64::from(counter) * 3000, 2));
            pes.extend_from_slice(&es);
            pes
        };
        let mut stream = packet(0, true, 0, None, &pat());
        stream.extend(packet(4095, true, 0, None, &pmt));
        let mut cc = 0;
        for (counter, keyframe) in [(41, true), (42, false)] {
            let pes = au(keyframe, counter);
            let first = PACKET - 4 - 8;
            stream.extend(packet(256, true, cc, Some(80_000), &pes));
            stream.extend(packet(256, false, (cc + 1) & 0x0F, None, &pes[first..]));
            cc = (cc + 2) & 0x0F;
        }
        let mut reader = TsReader::new();
        let mut frames = Vec::new();
        for chunk in stream.chunks(77) {
            reader.feed(chunk, &mut |e| {
                if let Event::Video {
                    sei,
                    keyframe,
                    pts90,
                    ..
                } = e
                {
                    frames.push((sei.map(|f| f.frame_counter), keyframe, pts90));
                }
            });
        }
        assert_eq!(reader.video_codec, Some(VideoCodec::Hevc));
        assert_eq!(
            frames,
            vec![
                (Some(41), true, Some(90_000 + 41 * 3000)),
                (Some(42), false, Some(90_000 + 42 * 3000)),
            ]
        );
    }

    #[test]
    fn collects_whole_pes() {
        let mut stream = Vec::new();
        stream.extend(packet(0, true, 0, None, &pat()));
        stream.extend(packet(4095, true, 0, None, &pmt()));
        // An audio PES with its length set, split over two packets.
        let mut audio = vec![0, 0, 1, 0xC0, 0, 0, 0x84, 0x80, 5];
        audio.extend_from_slice(&ts_timestamp(5000, 2));
        audio.extend((0..250u32).map(|i| i as u8));
        let len = audio.len() - 6;
        audio[4] = (len >> 8) as u8;
        audio[5] = len as u8;
        stream.extend(packet(257, true, 0, None, &audio[..184]));
        stream.extend(packet(257, false, 1, None, &audio[184..]));
        let mut reader = TsReader::new();
        reader.collect_pes = true;
        let mut pes = Vec::new();
        reader.feed(&stream, &mut |e| {
            if let Event::Pes { video, pts90, es } = e {
                pes.push((video, pts90, es));
            }
        });
        assert_eq!(pes.len(), 1);
        assert!(!pes[0].0);
        assert_eq!(pes[0].1, Some(5000));
        assert_eq!(pes[0].2, (0..250u32).map(|i| i as u8).collect::<Vec<_>>());
    }

    #[test]
    fn opus_pmt_and_control_headers() {
        // The PMT TallyCam writes for H.264 and mono Opus.
        let pmt = psi(&[
            0x02, 0xB0, 0x21, 0, 1, 0xC1, 0, 0, 0xE1, 0x00, 0xF0, 0x00, // PCR PID 256
            0x1B, 0xE1, 0x00, 0xF0, 0x00, // H.264 on 256
            0x06, 0xE1, 0x01, 0xF0, 0x0A, // private data on 257, 10 bytes of descriptors
            0x05, 0x04, b'O', b'p', b'u', b's', 0x7F, 0x02, 0x80, 0x01, //
            0, 0, 0, 0,
        ]);
        let mut stream = packet(0, true, 0, None, &pat());
        stream.extend(packet(4095, true, 0, None, &pmt));
        let mut r = TsReader::new();
        r.feed(&stream, &mut |_| {});
        assert_eq!(r.audio_codec(), Some(AudioCodec::Opus { channels: 1 }));
        assert_eq!(r.audio_pid, Some(257));

        // Two access units, the second with 300 bytes and a start trim.
        let mut es = vec![0x7F, 0xE0, 3, 1, 2, 3];
        es.extend_from_slice(&[0x7F, 0xF0, 0xFF, 45, 0x01, 0x38]);
        es.extend(std::iter::repeat_n(9u8, 300));
        let packets = opus_packets(&es).unwrap();
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0], &[1, 2, 3]);
        assert_eq!(packets[1].len(), 300);
        assert!(opus_packets(&[0x12, 0x34]).is_none());
        assert!(opus_packets(&[0x7F, 0xE0, 10, 1]).is_none(), "short");
    }

    #[test]
    fn private_data_without_opus_is_not_audio() {
        let pmt = psi(&[
            0x02, 0xB0, 0x17, 0, 1, 0xC1, 0, 0, 0xE1, 0x00, 0xF0, 0x00, //
            0x1B, 0xE1, 0x00, 0xF0, 0x00, //
            0x06, 0xE1, 0x02, 0xF0, 0x00, // private data, no registration
            0, 0, 0, 0,
        ]);
        let mut stream = packet(0, true, 0, None, &pat());
        stream.extend(packet(4095, true, 0, None, &pmt));
        let mut r = TsReader::new();
        r.feed(&stream, &mut |_| {});
        assert_eq!(r.audio_codec(), None);
    }

    #[test]
    fn timestamp_round_trip() {
        for v in [0u64, 1, 90_000, (1 << 33) - 1, 0x1_2345_6789] {
            let b = ts_timestamp(v, 2);
            assert_eq!(read_timestamp(&b), v);
        }
    }

    #[test]
    fn reports_continuity_errors() {
        let mut reader = TsReader::new();
        let mut events = Vec::new();
        let mut stream = Vec::new();
        stream.extend(packet(0, true, 0, None, &pat()));
        stream.extend(packet(4095, true, 0, None, &pmt()));
        stream.extend(packet(257, false, 3, None, &[0; 184]));
        stream.extend(packet(257, false, 5, None, &[0; 184]));
        reader.feed(&stream, &mut |e| events.push(e));
        assert_eq!(events, vec![Event::Discontinuity { pid: 257 }]);
    }
}
