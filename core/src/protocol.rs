//! Live Contribution Protocol (LCP) v0: the device protocol.
//!
//! - [`DeviceMessage`]: phone -> server (`/lcp/v0/device`)
//! - [`ServerToDevice`]: server -> phone
//!
//! Unknown `type` values and unknown fields are ignored. Fields a server does
//! not interpret are kept in `extra` maps so they can be forwarded unchanged.
//! Controller-side envelopes (operator UI <-> server) belong to the server.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// LCP protocol version. 1 added device keys: `hello.public_key`, the
/// server's `challenge` and the device's `auth` (docs/PROTOCOL.md, "Device
/// keys"). A version 0 `hello` comes from an app without a key.
pub const PROTOCOL_VERSION: u32 = 1;

pub const DEVICE_PATH: &str = "/lcp/v0/device";
pub const CONTROL_PATH: &str = "/lcp/v0/control";

/// Map of control id to value, used by `state` and `set`.
pub type Values = Map<String, Value>;

// ---------------------------------------------------------------------------
// Descriptors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct DeviceInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub os_version: String,
    #[serde(default)]
    pub app_version: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    Range,
    Enum,
    Bool,
    Text,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Scale {
    #[default]
    Linear,
    Log,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnumOption {
    pub value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ControlDescriptor {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub kind: ControlKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale: Option<Scale>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<EnumOption>>,
    /// The control is writable only when the named controls have these values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_when: Option<Values>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub readonly: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ControlDescriptor {
    /// True when `enabled_when` is satisfied by `state` (and the control is not readonly).
    pub fn is_enabled(&self, state: &Values) -> bool {
        if self.readonly {
            return false;
        }
        match &self.enabled_when {
            None => true,
            Some(cond) => cond.iter().all(|(k, v)| state.get(k) == Some(v)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandDescriptor {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Argument name to a human-readable domain, e.g. `"0..1"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Map<String, Value>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

// ---------------------------------------------------------------------------
// Device -> server
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hello {
    #[serde(default)]
    pub protocol: u32,
    pub device: DeviceInfo,
    /// The device's P-256 public key, SEC1 uncompressed (65 bytes, X9.63),
    /// standard base64. The device id is derived from it. Absent from apps
    /// before device keys (protocol 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
    #[serde(default)]
    pub controls: Vec<ControlDescriptor>,
    #[serde(default)]
    pub commands: Vec<CommandDescriptor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Battery {
    pub level: f64,
    #[serde(default)]
    pub charging: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamStatus {
    /// `idle | connecting | live | error`
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bitrate_kbps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retransmits: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The SRT latency the listener agreed to (the larger of both ends), ms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub srt_latency_ms: Option<u32>,
    /// Send pipeline latency per stage over the last second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<Box<StreamLatency>>,
    /// Frames muxed after their PCR deadline (`PTS < PCR`) since the stream
    /// started: a receiver gets them late.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub late_video_frames: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub late_audio_frames: Option<u64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// p50, p95 and maximum of one stage over the last second, ms.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Percentiles {
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
}

/// `status.stream.latency`: where a frame's time goes on the phone, on its
/// host clock. Stages without samples in the last second are left out.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct StreamLatency {
    /// Camera capture time to the frame being handed to the encoder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_to_encode_ms: Option<Percentiles>,
    /// Encoder input to encoder output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encode_ms: Option<Percentiles>,
    /// Encoder output to MPEG-TS packets ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mux_ms: Option<Percentiles>,
    /// TS packets ready to the frame's first SRT packet sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_queue_ms: Option<Percentiles>,
    /// TS packets ready to the frame's last SRT packet sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub send_frame_ms: Option<Percentiles>,
    /// Camera capture time to the frame's first SRT packet sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_to_send_ms: Option<Percentiles>,
    /// Capture time of a microphone buffer's first sample to the buffer
    /// reaching the audio encoder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_capture_to_encode_ms: Option<Percentiles>,
    /// A frame's last sample into the audio encoder to its packet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_encode_ms: Option<Percentiles>,
    /// Audio PTS (capture time of the frame's first sample minus the encoder
    /// delay) to its TS packets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_capture_to_mux_ms: Option<Percentiles>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Status {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery: Option<Battery>,
    /// `nominal | fair | serious | critical`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thermal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<StreamStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// The phone's own clock discipline: tracking, health and sources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<Box<ClockStatus>>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Preview {
    /// Base64 encoded JPEG.
    pub jpeg: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandResult {
    pub id: Value,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Command output, e.g. the `clock_log` lines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Log {
    #[serde(default = "default_log_level")]
    pub level: String,
    pub message: String,
}

fn default_log_level() -> String {
    "info".into()
}

/// Application-level keepalive. Not in the v0 spec text; either side may send
/// `{"type":"ping"}` and gets `{"type":"pong"}` back with any `id` echoed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Ping {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeviceMessage {
    Hello(Hello),
    State {
        values: Values,
    },
    Status(Status),
    Preview(Preview),
    Result(CommandResult),
    Log(Log),
    /// Answer to `challenge`: the device key's signature over
    /// [`crate::device_key::auth_payload`], P-256 ECDSA with SHA-256,
    /// `r || s` (64 bytes), standard base64.
    Auth {
        signature: String,
    },
    Ping(Ping),
    Pong(Ping),
    #[serde(other)]
    Unknown,
}

// ---------------------------------------------------------------------------
// Server -> device
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TallyState {
    #[default]
    Off,
    Preview,
    Program,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerToDevice {
    Set {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<Value>,
        values: Values,
    },
    Command {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<Value>,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<Map<String, Value>>,
    },
    Tally {
        state: TallyState,
    },
    Message {
        text: String,
    },
    Preview {
        enabled: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fps: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
    },
    /// Sent after a `hello` that carries a public key: sign
    /// [`crate::device_key::auth_payload`]
    /// over these and answer with `auth`. `server` identifies the server
    /// instance, `nonce` is fresh for every challenge.
    Challenge {
        nonce: String,
        server: String,
    },
    Ping(Ping),
    Pong(Ping),
}

// ---------------------------------------------------------------------------
// status.clock
// ---------------------------------------------------------------------------

/// Verdict on the phone clock, the same model as Strom's clock health.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClockHealthLevel {
    Healthy,
    Degraded,
    #[default]
    Bad,
}

/// `status.clock`: the phone's chrony-like clock discipline, compact. Every
/// field has a default so a partial object from another implementation still
/// parses.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ClockStatus {
    /// At least `minsources` sources agree and the clock follows them.
    pub synced: bool,
    pub health: ClockHealthLevel,
    /// Operator-readable findings behind `health`.
    pub findings: Vec<String>,
    /// Timescale of the SEI capture times: `tai`.
    pub timescale: String,
    /// TAI - UTC in seconds.
    pub tai_utc_s: i32,
    /// Address of the selected source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Pool or server name the selected source came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_name: Option<String>,
    /// Our stratum (selected source stratum + 1); 0 when not synced.
    pub stratum: u8,
    /// Seconds since the last clock update.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub update_age_s: Option<f64>,
    /// Correction still being slewed in, ns; positive = clock fast.
    pub system_offset_ns: i64,
    /// Offset at the last update, ns; positive = clock fast.
    pub last_offset_ns: i64,
    pub rms_offset_ns: i64,
    /// Frequency correction of the host clock, ppm; positive = host slow.
    pub freq_ppm: f64,
    pub resid_freq_ppm: f64,
    pub skew_ppm: f64,
    pub root_delay_us: i64,
    pub root_dispersion_us: i64,
    pub update_interval_s: f64,
    /// `normal | insert | delete | unsynchronised`
    pub leap: String,
    pub est_error_us: i64,
    pub max_error_us: i64,
    /// Disciplined UTC minus the phone's own wall clock, ms.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_offset_ms: Option<f64>,
    pub selected: u32,
    pub combined: u32,
    pub reachable: u32,
    pub total: u32,
    pub sources: Vec<ClockSourceSummary>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One row of `chronyc sources`, compact.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ClockSourceSummary {
    /// `*` selected, `+` combined, `-` not combined, `x` falseticker,
    /// `~` too variable, `?` unusable or unreachable.
    pub state: String,
    pub name: String,
    pub addr: String,
    pub stratum: u8,
    /// log2 of the poll interval in seconds.
    pub poll: i8,
    /// Reachability register, last 8 polls (377 octal = all answered).
    pub reach: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_rx_s: Option<u64>,
    /// Last sample offset, us; positive = the reference is ahead of us.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_us: Option<i64>,
    /// Error bound of the last sample (root distance), us.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_us: Option<i64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_with_clock_round_trips_and_keeps_unknown_fields() {
        let text = r#"{"type":"status","thermal":"nominal","clock":{"synced":true,"health":"degraded",
            "max_error_us":4785,"sources":[{"state":"*","name":"ntp.ubuntu.com","reach":255,"future":1}],
            "later":"x"}}"#;
        let msg: DeviceMessage = serde_json::from_str(text).unwrap();
        let DeviceMessage::Status(status) = &msg else {
            panic!("not a status")
        };
        let clock = status.clock.as_ref().unwrap();
        assert!(clock.synced);
        assert_eq!(clock.health, ClockHealthLevel::Degraded);
        assert_eq!(clock.max_error_us, 4785);
        assert_eq!(clock.sources[0].reach, 255);
        assert_eq!(clock.extra["later"], "x");
        let back = serde_json::to_value(&msg).unwrap();
        assert_eq!(back["clock"]["sources"][0]["future"], 1);
    }

    #[test]
    fn stream_latency_round_trips() {
        let text = r#"{"type":"status","stream":{"state":"live","srt_latency_ms":120,
            "latency":{"encode_ms":{"p50":4.1,"p95":5.0,"max":7.2},
                       "capture_to_send_ms":{"p50":21.0,"p95":30.5,"max":41.0}},
            "late_video_frames":0,"late_audio_frames":2}}"#;
        let msg: DeviceMessage = serde_json::from_str(text).unwrap();
        let DeviceMessage::Status(status) = &msg else {
            panic!("not a status")
        };
        let stream = status.stream.as_ref().unwrap();
        assert_eq!(stream.srt_latency_ms, Some(120));
        let latency = stream.latency.as_ref().unwrap();
        assert_eq!(latency.encode_ms.unwrap().p95, 5.0);
        assert_eq!(latency.capture_to_send_ms.unwrap().max, 41.0);
        assert!(latency.mux_ms.is_none());
        assert_eq!(stream.late_audio_frames, Some(2));
        let back = serde_json::to_value(&msg).unwrap();
        assert_eq!(back["stream"]["latency"]["encode_ms"]["p50"], 4.1);
        assert!(back["stream"]["latency"].get("mux_ms").is_none());
    }

    #[test]
    fn time_request_is_now_unknown() {
        let msg: DeviceMessage =
            serde_json::from_str(r#"{"type":"time_request","id":1,"t1":5}"#).unwrap();
        assert_eq!(msg, DeviceMessage::Unknown);
    }

    #[test]
    fn device_key_messages_round_trip() {
        let hello: DeviceMessage = serde_json::from_str(
            r#"{"type":"hello","protocol":1,"device":{"id":"ab"},"public_key":"BAE="}"#,
        )
        .unwrap();
        let DeviceMessage::Hello(h) = &hello else {
            panic!("not a hello")
        };
        assert_eq!(h.public_key.as_deref(), Some("BAE="));
        let legacy: DeviceMessage =
            serde_json::from_str(r#"{"type":"hello","device":{"id":"ab"}}"#).unwrap();
        let DeviceMessage::Hello(h) = &legacy else {
            panic!("not a hello")
        };
        assert_eq!(h.public_key, None);
        assert!(!serde_json::to_string(&legacy)
            .unwrap()
            .contains("public_key"));

        let auth: DeviceMessage =
            serde_json::from_str(r#"{"type":"auth","signature":"c2ln"}"#).unwrap();
        assert_eq!(
            auth,
            DeviceMessage::Auth {
                signature: "c2ln".into()
            }
        );
        let challenge = ServerToDevice::Challenge {
            nonce: "n".into(),
            server: "s".into(),
        };
        assert_eq!(
            serde_json::to_value(&challenge).unwrap(),
            serde_json::json!({"type":"challenge","nonce":"n","server":"s"})
        );
    }

    #[test]
    fn result_carries_data() {
        let msg: DeviceMessage =
            serde_json::from_str(r#"{"type":"result","id":3,"ok":true,"data":{"lines":["a"]}}"#)
                .unwrap();
        let DeviceMessage::Result(r) = msg else {
            panic!()
        };
        assert_eq!(r.data.unwrap()["lines"][0], "a");
    }
}
