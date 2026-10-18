//! Length-prefixed NAL units (as VideoToolbox emits them) or Annex B NAL
//! units (as MediaCodec emits them) to the Annex B access units MPEG-TS
//! carries.
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

/// `length_size` meaning "the frame is already Annex B" (NAL units after
/// 3- or 4-byte start codes, as MediaCodec writes them).
pub const ANNEX_B: usize = 0;

/// Appends one access unit to `out`: AUD, `parameter_sets` (Annex B, start
/// codes included; `Some` on keyframes), the encoder's NAL units from
/// `frame` with start codes, and `sei` (one NAL unit without start code)
/// before the first slice.
///
/// `length_size` 1 to 4: each NAL unit of `frame` after a big-endian length
/// of that many bytes; a truncated or zero length ends the frame.
/// [`ANNEX_B`] (0): `frame` is Annex B ([`split`]); bytes before the first
/// start code and empty NAL units are skipped.
pub fn append_access_unit(
    out: &mut impl ByteSink,
    codec: VideoCodec,
    frame: &[u8],
    length_size: usize,
    parameter_sets: Option<&[u8]>,
    sei: Option<&[u8]>,
) {
    if length_size == ANNEX_B {
        append_nal_units(out, codec, split(frame), parameter_sets, sei);
    } else {
        append_nal_units(
            out,
            codec,
            length_prefixed(frame, length_size),
            parameter_sets,
            sei,
        );
    }
}

/// [`append_access_unit`] for NAL units already split (no start codes or
/// lengths; empty ones are skipped).
pub fn append_nal_units<'a>(
    out: &mut impl ByteSink,
    codec: VideoCodec,
    nal_units: impl Iterator<Item = &'a [u8]>,
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
    for nal in nal_units {
        let Some(&first) = nal.first() else {
            continue;
        };
        let t = nal_type(codec, first);
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

/// The NAL units of `bytes`, each after a big-endian length of
/// `length_size` (1 to 4) bytes. A truncated or zero length ends them.
pub fn length_prefixed(bytes: &[u8], length_size: usize) -> impl Iterator<Item = &[u8]> {
    let mut offset = 0;
    std::iter::from_fn(move || {
        if length_size == 0 || offset + length_size > bytes.len() {
            return None;
        }
        let length = bytes[offset..offset + length_size]
            .iter()
            .fold(0usize, |n, &b| n << 8 | usize::from(b));
        offset += length_size;
        if length == 0 || length > bytes.len() - offset {
            offset = bytes.len();
            return None;
        }
        let nal = &bytes[offset..offset + length];
        offset += length;
        Some(nal)
    })
}

/// Index of the first `00 00 01` at or after `from`, or None. memchr finds
/// the candidate `01` bytes (rare in coded data), then the two zeros before
/// it are checked.
#[inline]
fn find_start_code(bytes: &[u8], from: usize) -> Option<usize> {
    let mut i = from + 2;
    while i < bytes.len() {
        let k = i + memchr::memchr(1, &bytes[i..])?;
        if bytes[k - 1] == 0 && bytes[k - 2] == 0 {
            return Some(k - 2);
        }
        i = k + 1;
    }
    None
}

/// The NAL units of an Annex B byte stream (3- or 4-byte start codes),
/// without start codes. A NAL unit ends where the next start code begins
/// (the leading zero of a 4-byte one not included); one can be empty.
pub fn split(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    split_ranges(bytes).map(move |r| &bytes[r])
}

/// [`split`] as offsets into `bytes`.
pub fn split_ranges(bytes: &[u8]) -> impl Iterator<Item = std::ops::Range<usize>> + '_ {
    let mut next = find_start_code(bytes, 0);
    std::iter::from_fn(move || {
        let start = next? + 3;
        next = find_start_code(bytes, start);
        let end = match next {
            Some(j) if j > start && bytes[j - 1] == 0 => j - 1,
            Some(j) => j,
            None => bytes.len(),
        };
        Some(start..end)
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

    /// The same NAL units after start codes (alternating 4- and 3-byte).
    fn annexb_frame(nals: &[&[u8]]) -> Vec<u8> {
        let mut out = Vec::new();
        for (i, n) in nals.iter().enumerate() {
            out.extend_from_slice(if i % 2 == 0 { &S[..] } else { &S[1..] });
            out.extend_from_slice(n);
        }
        out
    }

    #[test]
    fn annexb_input_gives_the_same_access_unit_as_length_prefixed() {
        let h264: &[&[u8]] = &[
            &[0x09, 0xF0],
            &[0x67, 0x64, 0x00, 0x28, 0xAC],
            &[0x68, 0xEE, 0x3C, 0x80],
            &[0x06, 0x06, 0x01, 0xC4, 0x80],
            &[0x65, 0x88, 0x84, 0x00, 0x33, 0x00, 0x00, 0x03, 0x01],
            &[0x65, 0x00, 0x11],
        ];
        let hevc: &[&[u8]] = &[
            &[0x46, 0x01, 0x50],
            &[0x40, 0x01, 0x0C, 0x01, 0xFF],
            &[0x4E, 0x01, 0x06, 0x01, 0xC4, 0x80],
            &[0x26, 0x01, 0xAF, 0x00, 0x33],
            &[0x50, 0x01, 0x84, 0x80],
        ];
        for (codec, sei_codec, nals) in [
            (VideoCodec::H264, Codec::H264, h264),
            (VideoCodec::Hevc, Codec::Hevc, hevc),
        ] {
            let sei = sei(sei_codec);
            let sets = annexb(&nals[1..2]);
            for (sets, sei) in [
                (Some(&sets[..]), Some(&sei[..])),
                (None, Some(&sei[..])),
                (None, None),
            ] {
                let mut expected = Vec::new();
                append_access_unit(&mut expected, codec, &length_prefixed(nals), 4, sets, sei);
                let mut out = Vec::new();
                append_access_unit(&mut out, codec, &annexb_frame(nals), ANNEX_B, sets, sei);
                assert_eq!(out, expected, "{codec:?}");
                let mut count = ByteCount::default();
                append_access_unit(&mut count, codec, &annexb_frame(nals), ANNEX_B, sets, sei);
                assert_eq!(count.0, expected.len());
            }
        }
    }

    #[test]
    fn annexb_input_skips_leading_bytes_and_empty_nal_units() {
        let slice: &[u8] = &[0x41, 0x9A, 0x02];
        let input = cat(&[&[0xFF, 0x00], &S, &S, &S[1..], slice, &S]);
        let mut out = Vec::new();
        append_access_unit(&mut out, VideoCodec::H264, &input, ANNEX_B, None, None);
        assert_eq!(out, cat(&[&S, &[0x09, 0x30], &S, slice]));
        // No start code at all: the AUD only.
        out.clear();
        append_access_unit(&mut out, VideoCodec::H264, slice, ANNEX_B, None, None);
        assert_eq!(out, cat(&[&S, &[0x09, 0x30]]));
    }

    /// The byte-by-byte scan `split` had before it used memchr.
    fn split_reference(bytes: &[u8]) -> Vec<&[u8]> {
        let mut nals = Vec::new();
        let mut i = 0;
        loop {
            let start = loop {
                if i + 3 > bytes.len() {
                    return nals;
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
            nals.push(&bytes[start..end]);
        }
    }

    #[test]
    fn split_matches_the_byte_by_byte_scan() {
        // Bytes from {0, 1, 2, 0xAB}, zeros most likely: start codes, runs
        // of zeros and near misses everywhere.
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        for len in 0..400 {
            let bytes: Vec<u8> = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    [0, 0, 0, 1, 2, 0xAB][(x % 6) as usize]
                })
                .collect();
            assert_eq!(split(&bytes).collect::<Vec<_>>(), split_reference(&bytes));
        }
    }

    #[test]
    fn split_three_and_four_byte_start_codes() {
        let bytes = [0, 0, 0, 1, 0x67, 1, 0, 0, 1, 0x68, 2, 0, 0, 0, 1, 0x65];
        let nals: Vec<&[u8]> = split(&bytes).collect();
        assert_eq!(nals, [&[0x67, 1][..], &[0x68, 2][..], &[0x65][..]]);
        assert_eq!(split(&[0, 0]).count(), 0);
    }
}
