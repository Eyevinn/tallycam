//! Program specific information: the program association table (PAT) and
//! program map table (PMT), each in one TS packet.

use super::{PACKET_SIZE, PROGRAM_NUMBER};

const PAT_TABLE_ID: u8 = 0x00;
const PMT_TABLE_ID: u8 = 0x02;

/// One elementary stream entry of the PMT.
#[derive(Debug, Clone, Copy)]
pub struct Stream<'a> {
    pub stream_type: u8,
    pub pid: u16,
    /// ES descriptors, encoded (tag, length, data ...).
    pub descriptors: &'a [u8],
}

/// CRC-32/MPEG-2 (polynomial 0x04C11DB7, no reflection, no final xor).
pub fn crc32(data: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut i = 0;
        while i < 256 {
            let mut crc = (i as u32) << 24;
            let mut bit = 0;
            while bit < 8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04C1_1DB7
                } else {
                    crc << 1
                };
                bit += 1;
            }
            table[i] = crc;
            i += 1;
        }
        table
    };
    data.iter().fold(0xFFFF_FFFF, |crc, &b| {
        (crc << 8) ^ TABLE[((crc >> 24) ^ u32::from(b)) as usize & 0xFF]
    })
}

/// Writes a long-form section around `body` (written by the closure after
/// the 8-byte header) into `out`; returns its length.
fn section(
    out: &mut [u8],
    table_id: u8,
    table_id_extension: u16,
    body: impl FnOnce(&mut [u8]) -> usize,
) -> usize {
    let n = body(&mut out[8..]);
    // 5 bytes of syntax header after the length field, 4 bytes of CRC.
    let section_length = (n + 9) as u16;
    out[0] = table_id;
    // Section syntax indicator, '0', reserved '11'.
    out[1..3].copy_from_slice(&(0xB000 | section_length).to_be_bytes());
    out[3..5].copy_from_slice(&table_id_extension.to_be_bytes());
    // Reserved '11', version 0, current/next indicator 1.
    out[5] = 0xC1;
    // Section number, last section number.
    out[6] = 0;
    out[7] = 0;
    let end = 8 + n;
    let crc = crc32(&out[..end]);
    out[end..end + 4].copy_from_slice(&crc.to_be_bytes());
    end + 4
}

/// The PAT section of program 1 into `out`; returns its length.
pub fn pat_section(out: &mut [u8], pmt_pid: u16) -> usize {
    section(out, PAT_TABLE_ID, 1, |b| {
        b[0..2].copy_from_slice(&PROGRAM_NUMBER.to_be_bytes());
        b[2..4].copy_from_slice(&(0xE000 | pmt_pid).to_be_bytes());
        4
    })
}

/// The PMT section of program 1 into `out` (streams in PID order, no
/// program descriptors); returns its length.
pub fn pmt_section(out: &mut [u8], pcr_pid: u16, streams: &[Stream]) -> usize {
    section(out, PMT_TABLE_ID, PROGRAM_NUMBER, |b| {
        b[0..2].copy_from_slice(&(0xE000 | pcr_pid).to_be_bytes());
        b[2..4].copy_from_slice(&0xF000u16.to_be_bytes());
        let mut n = 4;
        let mut order = [0usize, 1];
        if streams.len() == 2 && streams[1].pid < streams[0].pid {
            order = [1, 0];
        }
        for &i in &order[..streams.len().min(2)] {
            let s = &streams[i];
            b[n] = s.stream_type;
            b[n + 1..n + 3].copy_from_slice(&(0xE000 | s.pid).to_be_bytes());
            b[n + 3..n + 5].copy_from_slice(&(0xF000 | s.descriptors.len() as u16).to_be_bytes());
            n += 5;
            b[n..n + s.descriptors.len()].copy_from_slice(s.descriptors);
            n += s.descriptors.len();
        }
        n
    })
}

/// One TS packet carrying one section (continuity counter 0; the muxer
/// patches it): payload unit start, pointer field 0, padding 0xFF.
pub fn write_packet(packet: &mut [u8], pid: u16, section: impl FnOnce(&mut [u8]) -> usize) {
    packet[..PACKET_SIZE].fill(0xFF);
    packet[0] = 0x47;
    packet[1] = 0x40 | (pid >> 8) as u8 & 0x1F;
    packet[2] = pid as u8;
    packet[3] = 0x10;
    packet[4] = 0;
    section(&mut packet[5..PACKET_SIZE]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ts::audio::opus_descriptors;

    #[test]
    fn crc_of_section_with_crc_is_zero() {
        assert_eq!(
            crc32(b"123456789"),
            0x0376_E6E7,
            "CRC-32/MPEG-2 check value"
        );
        let mut buf = [0u8; 183];
        let n = pat_section(&mut buf, 4095);
        assert_eq!(crc32(&buf[..n]), 0);
        assert_eq!(
            &buf[..n - 4],
            &[0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01, 0xEF, 0xFF]
        );
    }

    #[test]
    fn opus_pmt_entry() {
        let d = opus_descriptors(1);
        let mut buf = [0u8; 183];
        let n = pmt_section(
            &mut buf,
            256,
            &[
                Stream {
                    stream_type: 0x1B,
                    pid: 256,
                    descriptors: &[],
                },
                Stream {
                    stream_type: 0x06,
                    pid: 257,
                    descriptors: &d,
                },
            ],
        );
        assert_eq!(crc32(&buf[..n]), 0);
        assert_eq!(usize::from(buf[1] & 0x0F) << 8 | usize::from(buf[2]), n - 3);
        assert_eq!(&buf[8..12], &[0xE1, 0x00, 0xF0, 0x00], "PCR PID 256");
        assert_eq!(&buf[12..17], &[0x1B, 0xE1, 0x00, 0xF0, 0x00]);
        assert_eq!(
            &buf[17..n - 4],
            &[
                0x06, 0xE1, 0x01, 0xF0, 0x0A, 0x05, 0x04, 0x4F, 0x70, 0x75, 0x73, 0x7F, 0x02, 0x80,
                0x01
            ]
        );
    }
}
