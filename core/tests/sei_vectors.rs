//! The shared SEI byte vectors (`tests/vectors/sei_vectors.json`).

use serde::Deserialize;
use tallycam_core::sei::{self, Codec, Fields};

#[derive(Deserialize)]
struct Vector {
    name: String,
    capture_ns: u64,
    synced: bool,
    tai: bool,
    frame_counter: u32,
    uncertainty_us: u32,
    payload_hex: String,
    h264_nal_hex: String,
    hevc_nal_hex: String,
}

#[derive(Deserialize)]
struct File {
    vectors: Vec<Vector>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn sei_vectors() {
    let file: File = serde_json::from_str(include_str!("vectors/sei_vectors.json")).unwrap();
    assert!(file.vectors.len() >= 5);
    for v in file.vectors {
        let f = Fields {
            capture_ns: v.capture_ns,
            synced: v.synced,
            tai: v.tai,
            frame_counter: v.frame_counter,
            uncertainty_us: v.uncertainty_us,
        };
        assert_eq!(hex(&sei::payload(&f)), v.payload_hex, "{}", v.name);
        let mut buf = [0u8; sei::MAX_NAL_LEN];
        let n = sei::write_nal_unit(&f, Codec::H264, &mut buf);
        assert_eq!(hex(&buf[..n]), v.h264_nal_hex, "{}", v.name);
        assert_eq!(
            hex(&sei::nal_unit(&f, Codec::Hevc)),
            v.hevc_nal_hex,
            "{}",
            v.name
        );
        assert_eq!(
            sei::parse_nal_unit(&buf[..n]),
            Some((Codec::H264, f)),
            "{}",
            v.name
        );
    }
}
