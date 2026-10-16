//! Audio in MPEG-TS: AAC as ADTS, and Opus as GStreamer (`mpegtsmux`,
//! `tsdemux`) and FFmpeg carry it (the Opus-in-TS draft, ETSI TS 102 366
//! style descriptors):
//!
//! - stream_type 0x06 (PES private data), PES stream_id 0xBD
//!   (private_stream_1);
//! - ES descriptors: registration (tag 0x05) 'Opus', and the DVB extension
//!   descriptor (tag 0x7F) with extension tag 0x80 and the
//!   channel_config_code (1 mono, 2 stereo);
//! - every access unit (one Opus packet) prefixed with the control header:
//!   `0x7FE0` (11-bit prefix 0x3FF, no trim or extension flags) and
//!   `au_size` as a run of 0xFF bytes plus a final byte below 255.

pub const ADTS_HEADER_SIZE: usize = 7;
pub const OPUS_STREAM_ID: u8 = 0xBD;
/// Registration descriptor 'Opus' and the extension descriptor; the last
/// byte is the channel count.
pub const OPUS_DESCRIPTORS_LEN: usize = 10;

/// ADTS sampling frequency index (ISO/IEC 14496-3), `None` if the rate has
/// none.
pub fn adts_frequency_index(sample_rate: u32) -> Option<u8> {
    const RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    RATES
        .iter()
        .position(|&r| r == sample_rate)
        .map(|i| i as u8)
}

/// ADTS header for one AAC-LC frame of `length` bytes (header excluded):
/// MPEG-2 ID, no CRC, buffer fullness 0x7FF (VBR), one raw data block.
pub fn adts_header(frequency_index: u8, channels: u8, length: usize) -> [u8; ADTS_HEADER_SIZE] {
    const OBJECT_TYPE_AAC_LC: u8 = 2;
    let full = (ADTS_HEADER_SIZE + length) & 0x1FFF;
    [
        0xFF,
        // Sync, MPEG-2 ID, layer 0, protection absent.
        0xF9,
        (OBJECT_TYPE_AAC_LC - 1) << 6 | (frequency_index & 0x0F) << 2 | (channels >> 2) & 1,
        (channels & 3) << 6 | (full >> 11) as u8,
        ((full & 0x7FF) >> 3) as u8,
        ((full & 7) as u8) << 5 | 0x1F,
        0xFC,
    ]
}

/// Appends the Opus control header for an access unit of `au_size` bytes.
pub fn append_opus_control_header(out: &mut Vec<u8>, au_size: usize) {
    out.extend_from_slice(&[0x7F, 0xE0]);
    let mut n = au_size;
    while n >= 255 {
        out.push(255);
        n -= 255;
    }
    out.push(n as u8);
}

/// ES descriptors of the Opus PMT entry, for 1 or 2 channels (mapping
/// family 0; other counts are clamped).
pub fn opus_descriptors(channels: u8) -> [u8; OPUS_DESCRIPTORS_LEN] {
    [
        0x05,
        4,
        b'O',
        b'p',
        b'u',
        b's',
        0x7F,
        2,
        0x80,
        channels.clamp(1, 2),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_control_header() {
        let header = |n| {
            let mut v = Vec::new();
            append_opus_control_header(&mut v, n);
            v
        };
        assert_eq!(header(0), [0x7F, 0xE0, 0x00]);
        assert_eq!(header(120), [0x7F, 0xE0, 120]);
        assert_eq!(header(254), [0x7F, 0xE0, 254]);
        assert_eq!(header(255), [0x7F, 0xE0, 255, 0]);
        assert_eq!(header(300), [0x7F, 0xE0, 255, 45]);
        assert_eq!(header(510), [0x7F, 0xE0, 255, 255, 0]);
    }

    #[test]
    fn opus_descriptor_bytes() {
        assert_eq!(
            opus_descriptors(1),
            [0x05, 0x04, 0x4F, 0x70, 0x75, 0x73, 0x7F, 0x02, 0x80, 0x01]
        );
        assert_eq!(opus_descriptors(2)[9], 2);
        assert_eq!(opus_descriptors(6)[9], 2);
    }

    #[test]
    fn adts_header_fields() {
        assert_eq!(adts_frequency_index(48000), Some(3));
        assert_eq!(adts_frequency_index(44100), Some(4));
        assert_eq!(adts_frequency_index(47999), None);
        let h = adts_header(3, 1, 300);
        assert_eq!(h[0], 0xFF);
        assert_eq!(h[1] & 0xF6, 0xF0, "sync, layer 0");
        assert_eq!(h[1] & 1, 1, "no CRC");
        assert_eq!(h[2] >> 6, 1, "AAC-LC");
        assert_eq!((h[2] >> 2) & 0x0F, 3);
        let channels = (h[2] & 1) << 2 | h[3] >> 6;
        assert_eq!(channels, 1);
        let len = usize::from(h[3] & 3) << 11 | usize::from(h[4]) << 3 | usize::from(h[5] >> 5);
        assert_eq!(len, 307);
        assert_eq!(
            u16::from(h[5] & 0x1F) << 6 | u16::from(h[6] >> 2),
            0x7FF,
            "VBR"
        );
        assert_eq!(h[6] & 3, 0, "one raw data block");
    }
}
