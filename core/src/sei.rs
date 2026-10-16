//! The LCP capture timestamp SEI (docs/PROTOCOL.md, "Frame timestamps in the
//! video"): user data unregistered (payload type 5), one per coded frame,
//! H.264 (`nal_unit_type` 6) or HEVC prefix SEI (`nal_unit_type` 39).
//!
//! Building a NAL unit is allocation free ([`write_nal_unit`] writes into a
//! caller buffer), so it can run per frame.

/// Identifies LCP timestamps: 8E3D91EF-3490-42BC-90A2-097EEB07F1A6.
pub const UUID: [u8; 16] = [
    0x8E, 0x3D, 0x91, 0xEF, 0x34, 0x90, 0x42, 0xBC, 0x90, 0xA2, 0x09, 0x7E, 0xEB, 0x07, 0xF1, 0xA6,
];
pub const VERSION: u8 = 1;
pub const PAYLOAD_TYPE: usize = 5;
/// uuid(16) version(1) flags(1) capture(8) frame(4) uncertainty(4).
pub const PAYLOAD_SIZE: usize = 34;
pub const UNCERTAINTY_UNKNOWN: u32 = u32::MAX;
/// Flags bit 0: the capture clock was synchronized when the frame was stamped.
pub const FLAG_SYNCED: u8 = 0x01;
/// Flags bit 1: capture time is TAI ns since the PTP epoch (1970-01-01 TAI).
/// Without it, the capture time is UTC ns since the Unix epoch (older senders).
pub const FLAG_TAI: u8 = 0x02;
/// Upper bound of a timestamp SEI NAL unit, header and emulation prevention
/// included: 2 + 2 * (2 + 34 + 1) would be the absolute worst case.
pub const MAX_NAL_LEN: usize = 2 + 2 * (2 + PAYLOAD_SIZE + 1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// `nal_unit_type` 6, 1-byte NAL header.
    H264,
    /// Prefix SEI, `nal_unit_type` 39, 2-byte NAL header.
    Hevc,
}

/// The fields of one timestamp SEI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fields {
    /// TAI ns since the PTP epoch when `tai`, else UTC ns since the Unix epoch.
    pub capture_ns: u64,
    pub synced: bool,
    pub tai: bool,
    pub frame_counter: u32,
    /// Microseconds; [`UNCERTAINTY_UNKNOWN`] when unknown.
    pub uncertainty_us: u32,
}

impl Fields {
    pub fn flags(&self) -> u8 {
        (if self.synced { FLAG_SYNCED } else { 0 }) | (if self.tai { FLAG_TAI } else { 0 })
    }

    /// Microseconds, rounded up and saturating; `None` or negative becomes
    /// [`UNCERTAINTY_UNKNOWN`].
    pub fn uncertainty_us_from_ns(ns: Option<i64>) -> u32 {
        match ns {
            Some(ns) if ns >= 0 => {
                let us = ns / 1000 + i64::from(ns % 1000 != 0);
                u32::try_from(us).unwrap_or(UNCERTAINTY_UNKNOWN)
            }
            _ => UNCERTAINTY_UNKNOWN,
        }
    }
}

/// The 34 payload bytes, before emulation prevention.
pub fn payload(fields: &Fields) -> [u8; PAYLOAD_SIZE] {
    let mut p = [0u8; PAYLOAD_SIZE];
    p[..16].copy_from_slice(&UUID);
    p[16] = VERSION;
    p[17] = fields.flags();
    p[18..26].copy_from_slice(&fields.capture_ns.to_be_bytes());
    p[26..30].copy_from_slice(&fields.frame_counter.to_be_bytes());
    p[30..34].copy_from_slice(&fields.uncertainty_us.to_be_bytes());
    p
}

/// Parses the 34-byte payload (UUID first). `None` for another UUID, an
/// unknown version or a short payload.
pub fn parse_payload(p: &[u8]) -> Option<Fields> {
    if p.len() < PAYLOAD_SIZE || p[..16] != UUID {
        return None;
    }
    parse_user_data(&p[16..])
}

/// Parses the user data after the UUID, as GStreamer's
/// `GstVideoSEIUserDataUnregisteredMeta` hands it out.
pub fn parse_user_data(d: &[u8]) -> Option<Fields> {
    if d.len() < PAYLOAD_SIZE - 16 || d[0] != VERSION {
        return None;
    }
    Some(Fields {
        synced: d[1] & FLAG_SYNCED != 0,
        tai: d[1] & FLAG_TAI != 0,
        capture_ns: u64::from_be_bytes(d[2..10].try_into().ok()?),
        frame_counter: u32::from_be_bytes(d[10..14].try_into().ok()?),
        uncertainty_us: u32::from_be_bytes(d[14..18].try_into().ok()?),
    })
}

/// Writes the complete timestamp SEI NAL unit (header and emulation-prevented
/// RBSP, no start code or length prefix) into `out` and returns its length.
/// `out` must hold [`MAX_NAL_LEN`] bytes; returns 0 if it is too small.
pub fn write_nal_unit(fields: &Fields, codec: Codec, out: &mut [u8]) -> usize {
    if out.len() < MAX_NAL_LEN {
        return 0;
    }
    let mut rbsp = [0u8; 2 + PAYLOAD_SIZE + 1];
    rbsp[0] = PAYLOAD_TYPE as u8;
    rbsp[1] = PAYLOAD_SIZE as u8;
    rbsp[2..2 + PAYLOAD_SIZE].copy_from_slice(&payload(fields));
    rbsp[2 + PAYLOAD_SIZE] = 0x80;
    let mut n = write_header(codec, out);
    n += write_emulation_prevented(&rbsp, &mut out[n..]);
    n
}

/// The timestamp SEI NAL unit as a vector.
pub fn nal_unit(fields: &Fields, codec: Codec) -> Vec<u8> {
    let mut out = [0u8; MAX_NAL_LEN];
    let n = write_nal_unit(fields, codec, &mut out);
    out[..n].to_vec()
}

/// Any single-message SEI NAL unit, for other payload types and sizes.
pub fn nal_unit_for(payload_type: usize, payload: &[u8], codec: Codec) -> Vec<u8> {
    let mut rbsp = Vec::with_capacity(payload.len() + 8);
    append_ff_coded(payload_type, &mut rbsp);
    append_ff_coded(payload.len(), &mut rbsp);
    rbsp.extend_from_slice(payload);
    rbsp.push(0x80);
    let mut out = vec![0u8; 2 + rbsp.len() * 3 / 2 + 2];
    let mut n = write_header(codec, &mut out);
    n += write_emulation_prevented(&rbsp, &mut out[n..]);
    out.truncate(n);
    out
}

/// Finds and parses an LCP timestamp in an SEI NAL unit (no start code).
pub fn parse_nal_unit(nal: &[u8]) -> Option<(Codec, Fields)> {
    let (codec, header) = match nal.first()? {
        b if b & 0x1F == 6 => (Codec::H264, 1),
        b if (b >> 1) & 0x3F == 39 => (Codec::Hevc, 2),
        _ => return None,
    };
    let rbsp = remove_emulation_prevention(nal.get(header..)?);
    let mut i = 0;
    while i < rbsp.len() && rbsp[i] != 0x80 {
        let (ty, used) = read_ff_coded(&rbsp[i..])?;
        i += used;
        let (size, used) = read_ff_coded(&rbsp[i..])?;
        i += used;
        let body = rbsp.get(i..i + size)?;
        if ty == PAYLOAD_TYPE {
            if let Some(fields) = parse_payload(body) {
                return Some((codec, fields));
            }
        }
        i += size;
    }
    None
}

fn write_header(codec: Codec, out: &mut [u8]) -> usize {
    match codec {
        Codec::H264 => {
            // forbidden_zero_bit 0, nal_ref_idc 0, nal_unit_type 6.
            out[0] = 0x06;
            1
        }
        Codec::Hevc => {
            // forbidden_zero_bit 0, nal_unit_type 39, nuh_layer_id 0, nuh_temporal_id_plus1 1.
            out[0] = 39 << 1;
            out[1] = 0x01;
            2
        }
    }
}

/// Inserts `emulation_prevention_three_byte` (0x03) wherever two zero bytes
/// are followed by a byte <= 0x03. Returns the bytes written.
fn write_emulation_prevented(rbsp: &[u8], out: &mut [u8]) -> usize {
    let mut zeros = 0;
    let mut n = 0;
    for &byte in rbsp {
        if zeros >= 2 && byte <= 0x03 {
            out[n] = 0x03;
            n += 1;
            zeros = 0;
        }
        out[n] = byte;
        n += 1;
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    n
}

/// Removes emulation prevention bytes.
pub fn remove_emulation_prevention(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0;
    for &byte in data {
        if zeros >= 2 && byte == 0x03 {
            zeros = 0;
            continue;
        }
        out.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    out
}

fn append_ff_coded(mut value: usize, out: &mut Vec<u8>) {
    while value >= 255 {
        out.push(0xFF);
        value -= 255;
    }
    out.push(value as u8);
}

fn read_ff_coded(data: &[u8]) -> Option<(usize, usize)> {
    let mut value = 0;
    for (i, &b) in data.iter().enumerate() {
        value += b as usize;
        if b != 0xFF {
            return Some((value, i + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_layout() {
        let f = Fields {
            capture_ns: 0x1122_3344_5566_7788,
            synced: true,
            tai: true,
            frame_counter: 0xA1B2_C3D4,
            uncertainty_us: 900,
        };
        let p = payload(&f);
        assert_eq!(p[..16], UUID);
        assert_eq!(p[16], 1);
        assert_eq!(p[17], 0x03);
        assert_eq!(p[18..26], [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
        assert_eq!(p[26..30], [0xA1, 0xB2, 0xC3, 0xD4]);
        assert_eq!(p[30..34], [0, 0, 0x03, 0x84]);
        assert_eq!(parse_payload(&p), Some(f));
    }

    #[test]
    fn uncertainty_conversion() {
        assert_eq!(Fields::uncertainty_us_from_ns(None), u32::MAX);
        assert_eq!(Fields::uncertainty_us_from_ns(Some(900_000)), 900);
        assert_eq!(Fields::uncertainty_us_from_ns(Some(900_001)), 901);
        assert_eq!(Fields::uncertainty_us_from_ns(Some(i64::MAX)), u32::MAX);
        assert_eq!(Fields::uncertainty_us_from_ns(Some(-1)), u32::MAX);
    }

    #[test]
    fn round_trip_with_emulation_prevention_everywhere() {
        for shift in 0..64 {
            let f = Fields {
                capture_ns: 1u64 << shift,
                synced: shift % 2 == 0,
                tai: shift % 3 == 0,
                frame_counter: 0,
                uncertainty_us: 0,
            };
            for codec in [Codec::H264, Codec::Hevc] {
                let nal = nal_unit(&f, codec);
                assert!(nal.len() <= MAX_NAL_LEN);
                assert_eq!(parse_nal_unit(&nal), Some((codec, f)));
                let header = if codec == Codec::H264 { 1 } else { 2 };
                let rbsp = remove_emulation_prevention(&nal[header..]);
                let mut expected = vec![5, 34];
                expected.extend_from_slice(&payload(&f));
                expected.push(0x80);
                assert_eq!(rbsp, expected);
                // No start code emulation is left.
                assert!(!nal.windows(3).any(|w| w[0] == 0 && w[1] == 0 && w[2] <= 2));
            }
        }
    }

    #[test]
    fn small_buffer_is_refused() {
        let f = Fields {
            capture_ns: 1,
            synced: true,
            tai: true,
            frame_counter: 1,
            uncertainty_us: 1,
        };
        let mut out = [0u8; 10];
        assert_eq!(write_nal_unit(&f, Codec::H264, &mut out), 0);
    }

    #[test]
    fn payload_size_coding() {
        let nal = nal_unit_for(5, &[0x11; 300], Codec::H264);
        assert_eq!(nal[..4], [0x06, 0x05, 0xFF, 45]);
        let exact = nal_unit_for(5, &[0x11; 255], Codec::H264);
        assert_eq!(exact[..4], [0x06, 0x05, 0xFF, 0x00]);
    }
}
