//! TS packets and PES packets (ISO/IEC 13818-1), written into reusable
//! buffers.

use super::PACKET_SIZE;

const SYNC: u8 = 0x47;
/// Payload bytes of a packet without adaptation field.
const FULL_PAYLOAD: usize = PACKET_SIZE - 4;
/// Bytes of the PES header written by [`begin_pes`]: start code, stream id,
/// length, flags, header length and the PTS.
pub const PES_HEADER_SIZE: usize = 14;

/// A 33-bit PTS or DTS with its 4-bit prefix (`0010` PTS only, `0011` PTS
/// with DTS, `0001` DTS) and marker bits.
pub fn timestamp(value: u64, prefix: u8) -> [u8; 5] {
    let b = value & 0x1_FFFF_FFFF;
    [
        prefix << 4 | ((b >> 29) as u8 & 0x0E) | 0x01,
        (b >> 22) as u8,
        ((b >> 14) as u8 & 0xFE) | 0x01,
        (b >> 7) as u8,
        ((b << 1) as u8) | 0x01,
    ]
}

/// Program clock reference: 33-bit base (90 kHz), 6 reserved bits, 9-bit
/// extension (27 MHz, always 0 here).
pub fn pcr(base: u64) -> [u8; 6] {
    let b = base & 0x1_FFFF_FFFF;
    [
        (b >> 25) as u8,
        (b >> 17) as u8,
        (b >> 9) as u8,
        (b >> 1) as u8,
        ((b & 1) as u8) << 7 | 0x7E,
        0,
    ]
}

/// Starts a PES packet in `pes` (cleared): a PTS and no DTS (no frame
/// reordering), data alignment indicator set. The length is filled in by
/// [`finish_pes`] once the payload is appended.
pub fn begin_pes(pes: &mut Vec<u8>, stream_id: u8, pts: u64) {
    pes.clear();
    // Marker bits '10', data alignment indicator; PTS only; 5 header bytes.
    pes.extend_from_slice(&pes_header(stream_id, pts, 0));
}

/// Sets PES_packet_length; 0 (unbounded) when the packet is too long for
/// the 16-bit field, as video PES may be.
pub fn finish_pes(pes: &mut [u8]) {
    let length = pes.len() - 6;
    let field = if length < usize::from(u16::MAX) {
        length as u16
    } else {
        0
    };
    pes[4..6].copy_from_slice(&field.to_be_bytes());
}

fn header(ts: &mut Vec<u8>, pid: u16, pusi: bool, adaptation: bool, cc: &mut u8) {
    ts.extend_from_slice(&[
        SYNC,
        if pusi { 0x40 } else { 0 } | (pid >> 8) as u8 & 0x1F,
        pid as u8,
        if adaptation { 0x20 } else { 0 } | 0x10 | *cc,
    ]);
    *cc = (*cc + 1) & 0x0F;
}

/// An adaptation field with only flags and stuffing.
fn stuffing_field(ts: &mut Vec<u8>, stuffing: usize) {
    ts.push((1 + stuffing) as u8);
    ts.push(0);
    ts.resize(ts.len() + stuffing, 0xFF);
}

/// Splits a PES packet into TS packets appended to `ts`.
///
/// The first packet always has an adaptation field (random access
/// indicator, the PCR if given, stuffing when the PES is short). Then full
/// 184-byte payloads, and a last packet padded with adaptation field
/// stuffing; a remainder of 183 bytes, which cannot take a 2-byte
/// adaptation field, goes out as 182 + 1.
pub fn packetize(
    pes: &[u8],
    pid: u16,
    random_access: bool,
    pcr_base: Option<u64>,
    cc: &mut u8,
    ts: &mut Vec<u8>,
) {
    let mut w = PesWriter::new(ts, pid, random_access, pcr_base, cc, pes.len());
    w.write(pes);
}

/// Writes a PES packet of a known length straight into TS packets as its
/// bytes arrive, laid out as [`packetize`] lays them out, so the PES never
/// has to be assembled in a buffer of its own.
pub struct PesWriter<'a> {
    ts: &'a mut Vec<u8>,
    pid: u16,
    cc: &'a mut u8,
    /// PES bytes still to come.
    left: usize,
    /// Payload bytes left in the current TS packet.
    room: usize,
}

impl<'a> PesWriter<'a> {
    /// Starts the first TS packet of a PES of `pes_len` bytes.
    pub fn new(
        ts: &'a mut Vec<u8>,
        pid: u16,
        random_access: bool,
        pcr_base: Option<u64>,
        cc: &'a mut u8,
        pes_len: usize,
    ) -> Self {
        let field = 2 + if pcr_base.is_some() { 6 } else { 0 };
        let max = FULL_PAYLOAD - field;
        let take = max.min(pes_len);
        let stuffing = max - take;
        header(ts, pid, true, true, cc);
        ts.push((field - 1 + stuffing) as u8);
        ts.push(if random_access { 0x40 } else { 0 } | if pcr_base.is_some() { 0x10 } else { 0 });
        if let Some(base) = pcr_base {
            ts.extend_from_slice(&pcr(base));
        }
        ts.resize(ts.len() + stuffing, 0xFF);
        PesWriter {
            ts,
            pid,
            cc,
            left: pes_len,
            room: take,
        }
    }

    /// Appends PES bytes. Bytes past the length given to [`Self::new`] are
    /// dropped.
    pub fn write(&mut self, data: &[u8]) {
        debug_assert!(data.len() <= self.left, "PES longer than announced");
        let mut data = &data[..data.len().min(self.left)];
        while !data.is_empty() {
            if self.room == 0 {
                self.next_packet();
            }
            let n = self.room.min(data.len());
            self.ts.extend_from_slice(&data[..n]);
            self.room -= n;
            self.left -= n;
            data = &data[n..];
        }
    }

    /// Bytes still to be written.
    pub fn left(&self) -> usize {
        self.left
    }

    /// Starts the packet after a full one: full 184-byte payloads while at
    /// least that much is left, then the padded last packet(s).
    fn next_packet(&mut self) {
        let rest = self.left;
        if rest >= FULL_PAYLOAD {
            header(self.ts, self.pid, false, false, self.cc);
            self.room = FULL_PAYLOAD;
        } else if rest == FULL_PAYLOAD - 1 {
            header(self.ts, self.pid, false, true, self.cc);
            stuffing_field(self.ts, 0);
            self.room = FULL_PAYLOAD - 2;
        } else {
            header(self.ts, self.pid, false, true, self.cc);
            stuffing_field(self.ts, FULL_PAYLOAD - 2 - rest);
            self.room = rest;
        }
    }
}

impl super::annexb::ByteSink for PesWriter<'_> {
    fn put(&mut self, bytes: &[u8]) {
        self.write(bytes);
    }
}

/// The PES header [`begin_pes`] and [`finish_pes`] write, for a PES of
/// `payload_len` bytes after the header.
pub fn pes_header(stream_id: u8, pts: u64, payload_len: usize) -> [u8; PES_HEADER_SIZE] {
    let length = PES_HEADER_SIZE - 6 + payload_len;
    let field = if length < usize::from(u16::MAX) {
        length as u16
    } else {
        0
    };
    let [l0, l1] = field.to_be_bytes();
    let t = timestamp(pts, 0x2);
    [
        0x00,
        0x00,
        0x01,
        stream_id,
        l0,
        l1,
        0x80 | 0x04,
        0x80,
        5,
        t[0],
        t[1],
        t[2],
        t[3],
        t[4],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Written in pieces as an access unit arrives, a PES comes out exactly
    /// as `packetize` lays out the whole PES, at every length around the
    /// packet boundaries (including the 183-byte remainder).
    #[test]
    fn pes_writer_in_pieces_matches_packetize() {
        let pes: Vec<u8> = (0..5 * FULL_PAYLOAD + 7).map(|i| i as u8).collect();
        for len in 0..pes.len() {
            for pcr_base in [None, Some(12_345)] {
                let (mut cc_a, mut cc_b) = (3, 3);
                let (mut whole, mut pieces) = (Vec::new(), Vec::new());
                packetize(&pes[..len], 256, true, pcr_base, &mut cc_a, &mut whole);
                let mut w = PesWriter::new(&mut pieces, 256, true, pcr_base, &mut cc_b, len);
                let mut at = 0;
                for step in [1, 4, 183, 2, 184, 185, 13].iter().cycle() {
                    if at == len {
                        break;
                    }
                    let n = (*step).min(len - at);
                    w.write(&pes[at..at + n]);
                    at += n;
                }
                assert_eq!(w.left(), 0);
                assert_eq!(pieces, whole, "len {len} pcr {pcr_base:?}");
                assert_eq!(cc_a, cc_b);
                assert_eq!(whole.len() % PACKET_SIZE, 0);
            }
        }
    }

    #[test]
    fn pes_header_matches_begin_and_finish() {
        for payload in [0, 5, 65_000, 70_000] {
            let mut pes = Vec::new();
            begin_pes(&mut pes, 0xE0, 0x1_2345_6789);
            pes.resize(pes.len() + payload, 0);
            finish_pes(&mut pes);
            assert_eq!(
                pes_header(0xE0, 0x1_2345_6789, payload)[..],
                pes[..PES_HEADER_SIZE]
            );
        }
    }

    fn read_timestamp(b: &[u8]) -> u64 {
        (u64::from(b[0] >> 1) & 0x07) << 30
            | u64::from(b[1]) << 22
            | (u64::from(b[2]) >> 1) << 15
            | u64::from(b[3]) << 7
            | u64::from(b[4]) >> 1
    }

    #[test]
    fn timestamps_round_trip_with_marker_bits() {
        for v in [
            0u64,
            1,
            90_000,
            (1 << 33) - 1,
            0x1_2345_6789,
            1 << 30,
            1 << 29,
        ] {
            let t = timestamp(v, 0x2);
            assert_eq!(t[0] & 0xF1, 0x21);
            assert_eq!(t[2] & 1, 1);
            assert_eq!(t[4] & 1, 1);
            assert_eq!(read_timestamp(&t), v & 0x1_FFFF_FFFF);
        }
    }

    #[test]
    fn pcr_layout() {
        let p = pcr(0x1_8000_0001);
        let base = u64::from(p[0]) << 25
            | u64::from(p[1]) << 17
            | u64::from(p[2]) << 9
            | u64::from(p[3]) << 1
            | u64::from(p[4]) >> 7;
        assert_eq!(base, 0x1_8000_0001);
        assert_eq!(p[4] & 0x7E, 0x7E, "reserved bits set");
        assert_eq!((p[4] & 1, p[5]), (0, 0), "extension 0");
    }

    /// Every PES length lands in whole packets with the right CC run and
    /// the payload back in order, including the 183-byte remainder.
    #[test]
    fn packetize_any_length() {
        for len in (1..1200).chain([184 * 3 + 176 + 183, 65_000, 70_000]) {
            for pcr_base in [None, Some(12345)] {
                let pes: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
                let mut ts = Vec::new();
                let mut cc = 14;
                packetize(&pes, 256, true, pcr_base, &mut cc, &mut ts);
                assert_eq!(ts.len() % PACKET_SIZE, 0, "len {len}");
                let n = ts.len() / PACKET_SIZE;
                assert_eq!(usize::from(cc), (14 + n) % 16);
                let mut back = Vec::new();
                for (k, p) in ts.chunks(PACKET_SIZE).enumerate() {
                    assert_eq!(p[0], SYNC);
                    assert_eq!(p[1] & 0x40 != 0, k == 0, "PUSI only first");
                    assert_eq!(u16::from(p[1] & 0x1F) << 8 | u16::from(p[2]), 256);
                    assert_eq!(usize::from(p[3] & 0x0F), (14 + k) % 16);
                    assert_eq!(p[3] & 0x10, 0x10, "payload");
                    let mut i = 4;
                    if p[3] & 0x20 != 0 {
                        let af = usize::from(p[4]);
                        if k == 0 {
                            assert_eq!(p[5] & 0x40, 0x40, "random access");
                            assert_eq!(p[5] & 0x10 != 0, pcr_base.is_some());
                        }
                        let fixed = if k == 0 && pcr_base.is_some() { 7 } else { 1 };
                        assert!(p[5 + fixed..5 + af].iter().all(|&b| b == 0xFF));
                        i = 5 + af;
                    }
                    assert!(i < PACKET_SIZE, "no empty payloads");
                    back.extend_from_slice(&p[i..]);
                }
                assert_eq!(back, pes, "len {len}");
            }
        }
    }

    #[test]
    fn pes_length_field() {
        let mut pes = Vec::new();
        begin_pes(&mut pes, 0xE0, 90_000);
        assert_eq!(pes.len(), PES_HEADER_SIZE);
        pes.extend_from_slice(&[1, 2, 3]);
        finish_pes(&mut pes);
        assert_eq!(&pes[..9], &[0, 0, 1, 0xE0, 0, 11, 0x84, 0x80, 5]);
        pes.resize(6 + 65_534, 0);
        finish_pes(&mut pes);
        assert_eq!(&pes[4..6], &[0xFF, 0xFE]);
        pes.push(0);
        finish_pes(&mut pes);
        assert_eq!(&pes[4..6], &[0, 0], "unbounded");
    }
}
