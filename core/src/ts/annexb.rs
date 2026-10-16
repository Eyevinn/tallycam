//! Length-prefixed NAL units (as VideoToolbox emits them) to the Annex B
//! access units MPEG-TS carries.
//!
//! Each access unit starts with an access unit delimiter (H.222.0 requires
//! one in every HEVC access unit; H.264 receivers find frame boundaries with
//! it). Keyframes get the parameter sets next. The encoder's own AUD and
//! parameter sets are dropped (ours come first), its other non-VCL NAL units
//! (SEI) are kept, and the LCP timestamp SEI goes right before the first
//! slice.

use super::VideoCodec;

pub const START_CODE: [u8; 4] = [0, 0, 0, 1];

pub const H264_SEI: u8 = 6;
pub const H264_SPS: u8 = 7;
pub const H264_PPS: u8 = 8;
pub const H264_AUD: u8 = 9;
pub const HEVC_VPS: u8 = 32;
pub const HEVC_SPS: u8 = 33;
pub const HEVC_PPS: u8 = 34;
pub const HEVC_AUD: u8 = 35;
pub const HEVC_PREFIX_SEI: u8 = 39;

/// H.264 AUD, primary_pic_type 0 (I slices only) and 1 (I and P slices).
const H264_AUD_INTRA: [u8; 2] = [0x09, 0x10];
const H264_AUD_INTER: [u8; 2] = [0x09, 0x30];
/// HEVC AUD (type 35, layer 0, temporal id 1), pic_type 0 (I) and 1 (P, I).
const HEVC_AUD_INTRA: [u8; 3] = [0x46, 0x01, 0x10];
const HEVC_AUD_INTER: [u8; 3] = [0x46, 0x01, 0x30];

/// `nal_unit_type` from the first byte of a NAL unit header.
pub fn nal_type(codec: VideoCodec, first: u8) -> u8 {
    match codec {
        VideoCodec::H264 => first & 0x1F,
        VideoCodec::Hevc => (first >> 1) & 0x3F,
    }
}

fn is_parameter_set_or_aud(codec: VideoCodec, t: u8) -> bool {
    match codec {
        VideoCodec::H264 => matches!(t, H264_AUD | H264_SPS | H264_PPS),
        VideoCodec::Hevc => matches!(t, HEVC_AUD | HEVC_VPS | HEVC_SPS | HEVC_PPS),
    }
}

fn is_slice(codec: VideoCodec, t: u8) -> bool {
    match codec {
        // Slices are types 1 to 5.
        VideoCodec::H264 => (1..=5).contains(&t),
        // VCL NAL units are types 0 to 31.
        VideoCodec::Hevc => t <= 31,
    }
}

/// Where an access unit's bytes go: a buffer, a TS packetizer
/// ([`super::packet::PesWriter`]) or a length count ([`ByteCount`]).
pub trait ByteSink {
    fn put(&mut self, bytes: &[u8]);
}

impl ByteSink for Vec<u8> {
    fn put(&mut self, bytes: &[u8]) {
        self.extend_from_slice(bytes);
    }
}

/// Counts the bytes put, to size a PES before writing it.
#[derive(Debug, Default)]
pub struct ByteCount(pub usize);

impl ByteSink for ByteCount {
    fn put(&mut self, bytes: &[u8]) {
        self.0 += bytes.len();
    }
}

/// Appends one access unit to `out`: AUD, `parameter_sets` (Annex B, start
/// codes included; `Some` on keyframes), the encoder's NAL units from
/// `length_prefixed` (each after a big-endian length of `length_size`
/// bytes) with start codes, and `sei` (one NAL unit without start code)
/// before the first slice. A truncated or zero length ends the frame.
pub fn append_access_unit(
    out: &mut impl ByteSink,
    codec: VideoCodec,
    length_prefixed: &[u8],
    length_size: usize,
    parameter_sets: Option<&[u8]>,
    sei: Option<&[u8]>,
) {
    out.put(&START_CODE);
    let keyframe = parameter_sets.is_some();
    out.put(match (codec, keyframe) {
        (VideoCodec::H264, true) => &H264_AUD_INTRA[..],
        (VideoCodec::H264, false) => &H264_AUD_INTER[..],
        (VideoCodec::Hevc, true) => &HEVC_AUD_INTRA[..],
        (VideoCodec::Hevc, false) => &HEVC_AUD_INTER[..],
    });
    if let Some(sets) = parameter_sets {
        out.put(sets);
    }
    let mut pending_sei = sei;
    let bytes = length_prefixed;
    let mut offset = 0;
    while offset + length_size <= bytes.len() {
        let length = bytes[offset..offset + length_size]
            .iter()
            .fold(0usize, |n, &b| n << 8 | usize::from(b));
        offset += length_size;
        if length == 0 || offset + length > bytes.len() {
            break;
        }
        let nal = &bytes[offset..offset + length];
        offset += length;
        let t = nal_type(codec, nal[0]);
        if is_parameter_set_or_aud(codec, t) {
            continue;
        }
        if is_slice(codec, t) {
            if let Some(sei) = pending_sei.take() {
                out.put(&START_CODE);
                out.put(sei);
            }
        }
        out.put(&START_CODE);
        out.put(nal);
    }
}

/// The NAL units of an Annex B byte stream (3- or 4-byte start codes),
/// without start codes.
pub fn split(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut i = 0;
    std::iter::from_fn(move || {
        // Find the next start code.
        let start = loop {
            if i + 3 > bytes.len() {
                return None;
            }
            if bytes[i] == 0 && bytes[i + 1] == 0 && bytes[i + 2] == 1 {
                break i + 3;
            }
            i += 1;
        };
        let mut j = start;
        let end = loop {
            if j + 3 > bytes.len() {
                break bytes.len();
            }
            if bytes[j] == 0 && bytes[j + 1] == 0 && bytes[j + 2] == 1 {
                break if bytes[j - 1] == 0 { j - 1 } else { j };
            }
            j += 1;
        };
        i = j;
        Some(&bytes[start..end])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sei::{self, Codec, Fields};

    const S: [u8; 4] = START_CODE;

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

    fn annexb(sets: &[&[u8]]) -> Vec<u8> {
        sets.iter()
            .flat_map(|s| S.iter().chain(s.iter()).copied())
            .collect()
    }

    fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.concat()
    }

    fn sei(codec: Codec) -> Vec<u8> {
        sei::nal_unit(
            &Fields {
                capture_ns: 42,
                synced: true,
                tai: true,
                frame_counter: 1,
                uncertainty_us: 5,
            },
            codec,
        )
    }

    #[test]
    fn h264_sei_position_in_idr_access_unit() {
        let sps: &[u8] = &[0x67, 0x64, 0x00, 0x28, 0xAC];
        let pps: &[u8] = &[0x68, 0xEE, 0x3C, 0x80];
        let sei = sei(Codec::H264);
        let encoder_aud: &[u8] = &[0x09, 0xF0];
        let encoder_sei: &[u8] = &[0x06, 0x06, 0x01, 0xC4, 0x80];
        let idr: &[u8] = &[0x65, 0x88, 0x84, 0x00, 0x33];
        let idr2: &[u8] = &[0x65, 0x00, 0x11];
        let input = length_prefixed(&[encoder_aud, sps, pps, encoder_sei, idr, idr2]);
        let mut out = Vec::new();
        let sets = annexb(&[sps, pps]);
        append_access_unit(
            &mut out,
            VideoCodec::H264,
            &input,
            4,
            Some(&sets),
            Some(&sei),
        );
        let expected = cat(&[
            &S,
            &[0x09, 0x10],
            &S,
            sps,
            &S,
            pps,
            &S,
            encoder_sei,
            &S,
            &sei,
            &S,
            idr,
            &S,
            idr2,
        ]);
        assert_eq!(out, expected);
        let types: Vec<u8> = split(&out).map(|n| n[0] & 0x1F).collect();
        assert_eq!(types, [9, 7, 8, 6, 6, 5, 5]);
    }

    #[test]
    fn h264_p_frame() {
        let sei = sei(Codec::H264);
        let slice: &[u8] = &[0x41, 0x9A, 0x02];
        let input = length_prefixed(&[slice]);
        let mut out = Vec::new();
        append_access_unit(&mut out, VideoCodec::H264, &input, 4, None, Some(&sei));
        assert_eq!(&out[..6], &[0, 0, 0, 1, 0x09, 0x30]);
        let types: Vec<u8> = split(&out).map(|n| n[0] & 0x1F).collect();
        assert_eq!(types, [9, 6, 1]);
        out.clear();
        append_access_unit(&mut out, VideoCodec::H264, &input, 4, None, None);
        assert_eq!(split(&out).map(|n| n[0] & 0x1F).collect::<Vec<_>>(), [9, 1]);
    }

    #[test]
    fn hevc_sei_position_in_idr_access_unit() {
        let vps: &[u8] = &[0x40, 0x01, 0x0C, 0x01, 0xFF];
        let sps: &[u8] = &[0x42, 0x01, 0x01, 0x01, 0x60];
        let pps: &[u8] = &[0x44, 0x01, 0xC1, 0x72];
        let sei = sei(Codec::Hevc);
        let encoder_aud: &[u8] = &[0x46, 0x01, 0x50];
        let encoder_sei: &[u8] = &[0x4E, 0x01, 0x06, 0x01, 0xC4, 0x80];
        // IDR_W_RADL (19), two slice segments, then a suffix SEI (40).
        let idr: &[u8] = &[0x26, 0x01, 0xAF, 0x00, 0x33];
        let idr2: &[u8] = &[0x26, 0x01, 0x11];
        let suffix_sei: &[u8] = &[0x50, 0x01, 0x84, 0x80];
        let input = length_prefixed(&[
            encoder_aud,
            vps,
            sps,
            pps,
            encoder_sei,
            idr,
            idr2,
            suffix_sei,
        ]);
        let mut out = Vec::new();
        let sets = annexb(&[vps, sps, pps]);
        append_access_unit(
            &mut out,
            VideoCodec::Hevc,
            &input,
            4,
            Some(&sets),
            Some(&sei),
        );
        let expected = cat(&[
            &S,
            &[0x46, 0x01, 0x10],
            &S,
            vps,
            &S,
            sps,
            &S,
            pps,
            &S,
            encoder_sei,
            &S,
            &sei,
            &S,
            idr,
            &S,
            idr2,
            &S,
            suffix_sei,
        ]);
        assert_eq!(out, expected);
        let types: Vec<u8> = split(&out)
            .map(|n| nal_type(VideoCodec::Hevc, n[0]))
            .collect();
        assert_eq!(types, [35, 32, 33, 34, 39, 39, 19, 19, 40]);
    }

    #[test]
    fn hevc_p_frame() {
        let sei = sei(Codec::Hevc);
        // TRAIL_R (1).
        let slice: &[u8] = &[0x02, 0x01, 0xD0, 0x02];
        let input = length_prefixed(&[slice]);
        let mut out = Vec::new();
        append_access_unit(&mut out, VideoCodec::Hevc, &input, 4, None, Some(&sei));
        assert_eq!(&out[..7], &[0, 0, 0, 1, 0x46, 0x01, 0x30]);
        let types: Vec<u8> = split(&out)
            .map(|n| nal_type(VideoCodec::Hevc, n[0]))
            .collect();
        assert_eq!(types, [35, 39, 1]);
    }

    #[test]
    fn truncated_input_ends_the_frame() {
        let slice: &[u8] = &[0x41, 0x9A, 0x02];
        let mut input = length_prefixed(&[slice, slice]);
        input.truncate(input.len() - 1);
        let mut out = Vec::new();
        append_access_unit(&mut out, VideoCodec::H264, &input, 4, None, None);
        assert_eq!(split(&out).count(), 2, "AUD and the first slice");
        // 2-byte lengths.
        let input = [0x00, 0x03, 0x41, 0x9A, 0x02];
        out.clear();
        append_access_unit(&mut out, VideoCodec::H264, &input, 2, None, None);
        assert_eq!(split(&out).last().unwrap(), slice);
    }

    #[test]
    fn split_three_and_four_byte_start_codes() {
        let bytes = [0, 0, 0, 1, 0x67, 1, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65];
        let nals: Vec<&[u8]> = split(&bytes).collect();
        assert_eq!(nals, [&[0x67, 1][..], &[0x68, 2][..], &[0x65][..]]);
        assert_eq!(split(&[0, 0]).count(), 0);
    }
}
