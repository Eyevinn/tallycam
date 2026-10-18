//! Live Contribution Protocol (LCP) v0: the device protocol.
//!
//! - [`DeviceMessage`]: phone -> server (`/lcp/v0/device`)
//! - [`ServerToDevice`]: server -> phone
//!
//! Unknown `type` values and unknown fields are ignored. Fields a server does
//! not interpret are kept in `extra` maps so they can be forwarded unchanged.
//! An enum value from the other side that this version does not know reads
//! as `Unknown` (or, for [`TallyState`], as off), and numbers and flags are
//! read leniently ([`lenient`]): one odd value never rejects the message.
//! Controller-side envelopes (operator UI <-> server) belong to the server.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// LCP protocol version. 1 added device keys: `hello.public_key`, the
/// server's `challenge` and the device's `auth` (docs/PROTOCOL.md, "Device
/// keys"). A version 0 `hello` comes from an app without a key. 2 added
/// the production settings (`production`) and the image statistics
/// (`stats`, `image_stats`), docs/SHADING.md, and in its released builds
/// (0.2.0) ownership of exposure, colour and focus. 3 is 2 without
/// ownership: the server and the operator both change any control. An
/// `ownership` message from a version 2 phone is an unknown type, ignored.
/// 4 binds `auth` to the host the device connected to (`auth.host`, in the
/// signed payload), so a server can not pass a challenge on to another.
pub const PROTOCOL_VERSION: u32 = 4;
/// The version whose phones (0.2.0) refuse exposure, colour and focus from
/// a server that does not own them: told to update the app.
pub const OWNERSHIP_PROTOCOL: u32 = 2;

pub const DEVICE_PATH: &str = "/lcp/v0/device";
pub const CONTROL_PATH: &str = "/lcp/v0/control";

/// Map of control id to value, used by `state` and `set`.
pub type Values = Map<String, Value>;

/// `deserialize_with` helpers so that a number or flag from another
/// implementation never rejects the whole message (docs/PROTOCOL.md,
/// "Transport"):
///
/// - An integer field takes any number with an integral value (`1920.0`),
///   saturated to the type's range (`"stratum": 256` reads as 255).
/// - A fraction (`120.5`), a non-number or `null` reads as the default: 0
///   for a plain field, absent for an `Option`.
/// - A float field takes any number; anything else reads as the default.
/// - A flag is `true` only for `true`; anything else reads as `false`.
///
/// A field without `#[serde(default)]` must still be present.
pub mod lenient {
    use serde::{Deserialize, Deserializer};
    use serde_json::Value;

    /// An integer type a JSON number saturates into.
    pub trait Int: Sized + Default {
        fn from_i64(x: i64) -> Self;
        fn from_u64(x: u64) -> Self;
        /// `x` is finite and integral; `as` saturates.
        fn from_f64(x: f64) -> Self;
    }

    macro_rules! int {
        ($($t:ty),*) => {$(
            impl Int for $t {
                fn from_i64(x: i64) -> Self {
                    <$t>::try_from(x).unwrap_or(if x < 0 { <$t>::MIN } else { <$t>::MAX })
                }
                fn from_u64(x: u64) -> Self {
                    <$t>::try_from(x).unwrap_or(<$t>::MAX)
                }
                fn from_f64(x: f64) -> Self {
                    x as $t
                }
            }
        )*};
    }
    int!(u8, i8, u32, i32, u64, i64);

    fn int_of<T: Int>(v: &Value) -> Option<T> {
        let Value::Number(n) = v else { return None };
        if let Some(x) = n.as_i64() {
            return Some(T::from_i64(x));
        }
        if let Some(x) = n.as_u64() {
            return Some(T::from_u64(x));
        }
        let x = n.as_f64()?;
        (x.is_finite() && x.fract() == 0.0).then(|| T::from_f64(x))
    }

    pub fn int<'de, D: Deserializer<'de>, T: Int>(d: D) -> Result<T, D::Error> {
        Ok(int_of(&Value::deserialize(d)?).unwrap_or_default())
    }

    pub fn opt_int<'de, D: Deserializer<'de>, T: Int>(d: D) -> Result<Option<T>, D::Error> {
        Ok(int_of(&Value::deserialize(d)?))
    }

    pub fn float<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        Ok(opt_float(d)?.unwrap_or_default())
    }

    pub fn opt_float<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        Ok(Value::deserialize(d)?.as_f64())
    }

    pub fn flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
        Ok(Value::deserialize(d)? == Value::Bool(true))
    }
}

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
    /// A scale this version does not know (from a newer phone): draw the
    /// control as linear. Forwarded as `"unknown"`.
    #[serde(other)]
    Unknown,
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
    #[serde(
        default,
        deserialize_with = "lenient::opt_float",
        skip_serializing_if = "Option::is_none"
    )]
    pub min: Option<f64>,
    #[serde(
        default,
        deserialize_with = "lenient::opt_float",
        skip_serializing_if = "Option::is_none"
    )]
    pub max: Option<f64>,
    #[serde(
        default,
        deserialize_with = "lenient::opt_float",
        skip_serializing_if = "Option::is_none"
    )]
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
    #[serde(
        default,
        deserialize_with = "lenient::flag",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub readonly: bool,
    /// Why it is `readonly`, for the UI ("no manual exposure on this
    /// camera"). Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readonly_reason: Option<String>,
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
    #[serde(default, deserialize_with = "lenient::int")]
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
    #[serde(deserialize_with = "lenient::float")]
    pub level: f64,
    #[serde(default, deserialize_with = "lenient::flag")]
    pub charging: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StreamStatus {
    /// `idle | connecting | live | error`
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(
        default,
        deserialize_with = "lenient::opt_float",
        skip_serializing_if = "Option::is_none"
    )]
    pub bitrate_kbps: Option<f64>,
    #[serde(
        default,
        deserialize_with = "lenient::opt_float",
        skip_serializing_if = "Option::is_none"
    )]
    pub rtt_ms: Option<f64>,
    #[serde(
        default,
        deserialize_with = "lenient::opt_int",
        skip_serializing_if = "Option::is_none"
    )]
    pub retransmits: Option<u64>,
    #[serde(
        default,
        deserialize_with = "lenient::opt_int",
        skip_serializing_if = "Option::is_none"
    )]
    pub dropped: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The SRT latency the listener agreed to (the larger of both ends), ms.
    #[serde(
        default,
        deserialize_with = "lenient::opt_int",
        skip_serializing_if = "Option::is_none"
    )]
    pub srt_latency_ms: Option<u32>,
    /// Send pipeline latency per stage over the last second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<Box<StreamLatency>>,
    /// Frames muxed after their PCR deadline (`PTS < PCR`) since the stream
    /// started: a receiver gets them late.
    #[serde(
        default,
        deserialize_with = "lenient::opt_int",
        skip_serializing_if = "Option::is_none"
    )]
    pub late_video_frames: Option<u64>,
    #[serde(
        default,
        deserialize_with = "lenient::opt_int",
        skip_serializing_if = "Option::is_none"
    )]
    pub late_audio_frames: Option<u64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// p50, p95 and maximum of one stage over the last second, ms.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Percentiles {
    #[serde(deserialize_with = "lenient::float")]
    pub p50: f64,
    #[serde(deserialize_with = "lenient::float")]
    pub p95: f64,
    #[serde(deserialize_with = "lenient::float")]
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
    #[serde(default, deserialize_with = "lenient::int")]
    pub width: u32,
    #[serde(default, deserialize_with = "lenient::int")]
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandResult {
    pub id: Value,
    #[serde(deserialize_with = "lenient::flag")]
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
    /// `r || s` (64 bytes), standard base64, and the host it signed
    /// ([`crate::device_key::auth_host`] of the URL it connected to).
    Auth {
        signature: String,
        host: String,
    },
    Ping(Ping),
    Pong(Ping),
    /// Exposure and colour statistics, only while the server asked for them.
    ImageStats(ImageStats),
    #[serde(other)]
    Unknown,
}

// ---------------------------------------------------------------------------
// Server -> device
// ---------------------------------------------------------------------------

/// The tally light. A state this version does not know (from a newer hub or
/// operator UI) reads as `Off`: rather no light than a red one that may be
/// wrong, and off is also what a phone shows without a hub. There is no
/// `Unknown` variant, so it is never sent on.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TallyState {
    #[default]
    Off,
    Preview,
    Program,
}

impl<'de> Deserialize<'de> for TallyState {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match Value::deserialize(d)? {
            Value::String(s) if s == "preview" => TallyState::Preview,
            Value::String(s) if s == "program" => TallyState::Program,
            _ => TallyState::Off,
        })
    }
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
        #[serde(deserialize_with = "lenient::flag")]
        enabled: bool,
        #[serde(
            default,
            deserialize_with = "lenient::opt_float",
            skip_serializing_if = "Option::is_none"
        )]
        fps: Option<f64>,
        #[serde(
            default,
            deserialize_with = "lenient::opt_int",
            skip_serializing_if = "Option::is_none"
        )]
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
    /// Production settings, after admission and when they change.
    Production(Production),
    /// Start or stop `image_stats` (default 1 per second, at most 4).
    Stats {
        #[serde(deserialize_with = "lenient::flag")]
        enabled: bool,
        #[serde(
            default,
            deserialize_with = "lenient::opt_float",
            skip_serializing_if = "Option::is_none"
        )]
        fps: Option<f64>,
    },
}

// ---------------------------------------------------------------------------
// Production (docs/SHADING.md)
// ---------------------------------------------------------------------------

/// The production's shutter. On the wire `"mains"`, `"free"` or the
/// denominator of a fixed flicker-free shutter (`100` for 1/100 s). An
/// unknown value reads as `mains`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShutterPolicy {
    /// One mains period: 1/50 at 50 Hz, 1/60 at 60 Hz.
    #[default]
    Mains,
    /// No mains-lit light in the picture: the shutter is free in manual
    /// exposure, and auto shortens it once ISO is at its minimum.
    Free,
    /// A fixed shutter, 1/x s.
    Fixed(u32),
}

impl Serialize for ShutterPolicy {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            ShutterPolicy::Mains => s.serialize_str("mains"),
            ShutterPolicy::Free => s.serialize_str("free"),
            ShutterPolicy::Fixed(x) => s.serialize_u32(*x),
        }
    }
}

impl<'de> Deserialize<'de> for ShutterPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match Value::deserialize(d)? {
            Value::String(s) if s == "free" => ShutterPolicy::Free,
            Value::Number(n) => match n.as_f64() {
                Some(x) if (1.0..=100_000.0).contains(&x) && x.fract() == 0.0 => {
                    ShutterPolicy::Fixed(x as u32)
                }
                _ => ShutterPolicy::Mains,
            },
            _ => ShutterPolicy::Mains,
        })
    }
}

/// Production-wide settings (docs/SHADING.md, "Shutter"): the same for
/// every camera of the production. A phone that is live keeps its current
/// settings and applies these when the stream stops.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Production {
    /// 50, 60, or 0 (no mains-lit light).
    #[serde(deserialize_with = "lenient::int")]
    pub mains_hz: u32,
    #[serde(default)]
    pub shutter: ShutterPolicy,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Production {
    /// Is the shutter free (set in manual exposure, shortened by auto ISO at
    /// the lowest ISO)? Under a `free` policy, and under `mains` with no
    /// mains-lit light (`mains_hz` 0): there is nothing to flicker against.
    pub fn shutter_free(&self) -> bool {
        match self.shutter {
            ShutterPolicy::Free => true,
            ShutterPolicy::Mains => self.mains_hz == 0,
            ShutterPolicy::Fixed(_) => false,
        }
    }
}

// ---------------------------------------------------------------------------
// image_stats (docs/SHADING.md, "Image statistics")
// ---------------------------------------------------------------------------

/// Luma of the encoded picture. Levels are 0..1 of the nominal range.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LumaStats {
    #[serde(deserialize_with = "lenient::float")]
    pub mean: f64,
    #[serde(deserialize_with = "lenient::float")]
    pub p01: f64,
    #[serde(deserialize_with = "lenient::float")]
    pub p50: f64,
    #[serde(deserialize_with = "lenient::float")]
    pub p99: f64,
    /// Share of the picture at the top of the range.
    #[serde(deserialize_with = "lenient::float")]
    pub clipped: f64,
    /// Share of the picture at the bottom of the range.
    #[serde(deserialize_with = "lenient::float")]
    pub crushed: f64,
    /// 32 bins from black to white, one byte each (relative to the fullest
    /// bin, 255), standard base64.
    pub histogram: String,
    /// 32 columns left to right, each 16 levels from black up, one byte each
    /// (share of the column, 255 = all of it), standard base64.
    pub waveform: String,
}

/// Colour of the near-grey pixels: their mean R'/G' and B'/G' (1.0 when
/// neutral) and their share of the picture. The ratios are absent when no
/// pixel is near grey.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ColourStats {
    #[serde(
        default,
        deserialize_with = "lenient::opt_float",
        skip_serializing_if = "Option::is_none"
    )]
    pub neutral_rg: Option<f64>,
    #[serde(
        default,
        deserialize_with = "lenient::opt_float",
        skip_serializing_if = "Option::is_none"
    )]
    pub neutral_bg: Option<f64>,
    #[serde(default, deserialize_with = "lenient::float")]
    pub neutral_fraction: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ImageStats {
    pub luma: LumaStats,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colour: Option<ColourStats>,
    /// Camera settings at the moment of measurement (`iso`, `shutter`,
    /// `wb_temperature`, `wb_tint`, `exposure_offset_ev`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<Values>,
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
    /// A level this version does not know (from a newer phone): no better
    /// than `Bad`. Forwarded as `"unknown"`.
    #[serde(other)]
    Unknown,
}

/// `status.clock`: the phone's chrony-like clock discipline, compact. Every
/// field has a default so a partial object from another implementation still
/// parses.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ClockStatus {
    /// At least `minsources` sources agree and the clock follows them.
    #[serde(deserialize_with = "lenient::flag")]
    pub synced: bool,
    pub health: ClockHealthLevel,
    /// Operator-readable findings behind `health`.
    pub findings: Vec<String>,
    /// Timescale of the SEI capture times: `tai`.
    pub timescale: String,
    /// TAI - UTC in seconds.
    #[serde(deserialize_with = "lenient::int")]
    pub tai_utc_s: i32,
    /// Address of the selected source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Pool or server name the selected source came from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_name: Option<String>,
    /// Our stratum (selected source stratum + 1); 0 when not synced.
    #[serde(deserialize_with = "lenient::int")]
    pub stratum: u8,
    /// Seconds since the last clock update.
    #[serde(
        deserialize_with = "lenient::opt_float",
        skip_serializing_if = "Option::is_none"
    )]
    pub update_age_s: Option<f64>,
    /// Correction still being slewed in, ns; positive = clock fast.
    #[serde(deserialize_with = "lenient::int")]
    pub system_offset_ns: i64,
    /// Offset at the last update, ns; positive = clock fast.
    #[serde(deserialize_with = "lenient::int")]
    pub last_offset_ns: i64,
    #[serde(deserialize_with = "lenient::int")]
    pub rms_offset_ns: i64,
    /// Frequency correction of the host clock, ppm; positive = host slow.
    #[serde(deserialize_with = "lenient::float")]
    pub freq_ppm: f64,
    #[serde(deserialize_with = "lenient::float")]
    pub resid_freq_ppm: f64,
    #[serde(deserialize_with = "lenient::float")]
    pub skew_ppm: f64,
    #[serde(deserialize_with = "lenient::int")]
    pub root_delay_us: i64,
    #[serde(deserialize_with = "lenient::int")]
    pub root_dispersion_us: i64,
    #[serde(deserialize_with = "lenient::float")]
    pub update_interval_s: f64,
    /// `normal | insert | delete | unsynchronised`
    pub leap: String,
    #[serde(deserialize_with = "lenient::int")]
    pub est_error_us: i64,
    #[serde(deserialize_with = "lenient::int")]
    pub max_error_us: i64,
    /// Disciplined UTC minus the phone's own wall clock, ms.
    #[serde(
        deserialize_with = "lenient::opt_float",
        skip_serializing_if = "Option::is_none"
    )]
    pub device_offset_ms: Option<f64>,
    #[serde(deserialize_with = "lenient::int")]
    pub selected: u32,
    #[serde(deserialize_with = "lenient::int")]
    pub combined: u32,
    #[serde(deserialize_with = "lenient::int")]
    pub reachable: u32,
    #[serde(deserialize_with = "lenient::int")]
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
    #[serde(deserialize_with = "lenient::int")]
    pub stratum: u8,
    /// log2 of the poll interval in seconds.
    #[serde(deserialize_with = "lenient::int")]
    pub poll: i8,
    /// Reachability register, last 8 polls (377 octal = all answered).
    #[serde(deserialize_with = "lenient::int")]
    pub reach: u8,
    #[serde(
        deserialize_with = "lenient::opt_int",
        skip_serializing_if = "Option::is_none"
    )]
    pub last_rx_s: Option<u64>,
    /// Last sample offset, us; positive = the reference is ahead of us.
    #[serde(
        deserialize_with = "lenient::opt_int",
        skip_serializing_if = "Option::is_none"
    )]
    pub offset_us: Option<i64>,
    /// Error bound of the last sample (root distance), us.
    #[serde(
        deserialize_with = "lenient::opt_int",
        skip_serializing_if = "Option::is_none"
    )]
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
            serde_json::from_str(r#"{"type":"auth","signature":"c2ln","host":"h"}"#).unwrap();
        assert_eq!(
            auth,
            DeviceMessage::Auth {
                signature: "c2ln".into(),
                host: "h".into(),
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
    fn production_round_trips_and_says_when_the_shutter_is_free() {
        for (text, shutter, free) in [
            (
                r#"{"type":"production","mains_hz":50,"shutter":"mains"}"#,
                ShutterPolicy::Mains,
                false,
            ),
            (
                r#"{"type":"production","mains_hz":0,"shutter":"mains"}"#,
                ShutterPolicy::Mains,
                true,
            ),
            (
                r#"{"type":"production","mains_hz":60,"shutter":"free"}"#,
                ShutterPolicy::Free,
                true,
            ),
            (
                r#"{"type":"production","mains_hz":0,"shutter":100}"#,
                ShutterPolicy::Fixed(100),
                false,
            ),
        ] {
            let msg: ServerToDevice = serde_json::from_str(text).unwrap();
            let ServerToDevice::Production(p) = &msg else {
                panic!("not a production")
            };
            assert_eq!(p.shutter, shutter);
            assert_eq!(p.shutter_free(), free, "{text}");
            let back: Value = serde_json::to_value(&msg).unwrap();
            assert_eq!(back, serde_json::from_str::<Value>(text).unwrap());
        }
        // Unknown shutters read as mains; unknown fields are kept.
        let msg: ServerToDevice = serde_json::from_str(
            r#"{"type":"production","mains_hz":50,"shutter":"fast","later":1}"#,
        )
        .unwrap();
        let ServerToDevice::Production(p) = msg else {
            panic!("not a production")
        };
        assert_eq!(p.shutter, ShutterPolicy::Mains);
        assert_eq!(p.extra["later"], 1);
    }

    #[test]
    fn ownership_from_an_older_phone_is_ignored() {
        let msg: DeviceMessage = serde_json::from_str(
            r#"{"type":"ownership","groups":{"exposure":{"owner":"remote","holder":{"id":"a"}}}}"#,
        )
        .unwrap();
        assert_eq!(msg, DeviceMessage::Unknown);
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

    fn hello_control(control: &str) -> ControlDescriptor {
        let text = format!(
            r#"{{"type":"hello","protocol":4,"device":{{"id":"ab"}},"controls":[{control}]}}"#
        );
        let DeviceMessage::Hello(mut h) = serde_json::from_str(&text).unwrap() else {
            panic!("not a hello")
        };
        h.controls.remove(0)
    }

    fn clock(fields: &str) -> ClockStatus {
        let text = format!(r#"{{"type":"status","clock":{{{fields}}}}}"#);
        let DeviceMessage::Status(s) = serde_json::from_str(&text).unwrap() else {
            panic!("not a status")
        };
        *s.clock.unwrap()
    }

    #[test]
    fn unknown_scale_does_not_reject_the_hello() {
        let c = hello_control(r#"{"id":"iso","kind":"range","min":25,"max":3200,"scale":"exp"}"#);
        assert_eq!(c.scale, Some(Scale::Unknown));
        assert_eq!(c.max, Some(3200.0));
        let c = hello_control(r#"{"id":"iso","kind":"range","scale":"log"}"#);
        assert_eq!(c.scale, Some(Scale::Log));
    }

    #[test]
    fn unknown_clock_health_keeps_the_status() {
        let c = clock(r#""synced":true,"health":"unknown","max_error_us":120"#);
        assert_eq!(c.health, ClockHealthLevel::Unknown);
        assert!(c.synced);
        assert_eq!(c.max_error_us, 120);
        assert_eq!(
            clock(r#""health":"later""#).health,
            ClockHealthLevel::Unknown
        );
        assert_eq!(
            clock(r#""health":"healthy""#).health,
            ClockHealthLevel::Healthy
        );
    }

    #[test]
    fn unknown_tally_reads_as_off() {
        for (state, want) in [
            ("\"program\"", TallyState::Program),
            ("\"preview\"", TallyState::Preview),
            ("\"off\"", TallyState::Off),
            ("\"program_iso\"", TallyState::Off),
            ("null", TallyState::Off),
            ("1", TallyState::Off),
        ] {
            let msg: ServerToDevice =
                serde_json::from_str(&format!(r#"{{"type":"tally","state":{state}}}"#)).unwrap();
            assert_eq!(msg, ServerToDevice::Tally { state: want }, "{state}");
        }
        // Sent as before.
        assert_eq!(
            serde_json::to_value(ServerToDevice::Tally {
                state: TallyState::Program
            })
            .unwrap(),
            serde_json::json!({"type":"tally","state":"program"})
        );
    }

    #[test]
    fn out_of_range_integer_saturates() {
        let c = clock(r#""stratum":256,"sources":[{"stratum":-3,"poll":200,"reach":1e3}]"#);
        assert_eq!(c.stratum, 255);
        assert_eq!(c.sources[0].stratum, 0);
        assert_eq!(c.sources[0].poll, 127);
        assert_eq!(c.sources[0].reach, 255);
        let c = clock(r#""max_error_us":1e30,"total":-1"#);
        assert_eq!(c.max_error_us, i64::MAX);
        assert_eq!(c.total, 0);
    }

    #[test]
    fn fractional_integer_is_dropped_not_the_message() {
        let msg: DeviceMessage = serde_json::from_str(
            r#"{"type":"status","stream":{"state":"live","srt_latency_ms":120.5,"dropped":3.0}}"#,
        )
        .unwrap();
        let DeviceMessage::Status(s) = msg else {
            panic!("not a status")
        };
        let stream = s.stream.unwrap();
        assert_eq!(stream.srt_latency_ms, None);
        assert_eq!(stream.dropped, Some(3));
        assert_eq!(stream.state, "live");
        assert_eq!(clock(r#""stratum":2.5,"synced":true"#).stratum, 0);
    }

    #[test]
    fn integral_float_is_an_integer() {
        let msg: DeviceMessage =
            serde_json::from_str(r#"{"type":"preview","jpeg":"","width":1920.0,"height":1080}"#)
                .unwrap();
        let DeviceMessage::Preview(p) = msg else {
            panic!("not a preview")
        };
        assert_eq!((p.width, p.height), (1920, 1080));
        let msg: ServerToDevice =
            serde_json::from_str(r#"{"type":"preview","enabled":true,"width":640.0}"#).unwrap();
        assert_eq!(
            msg,
            ServerToDevice::Preview {
                enabled: true,
                fps: None,
                width: Some(640)
            }
        );
        let hello: DeviceMessage =
            serde_json::from_str(r#"{"type":"hello","protocol":4.0,"device":{"id":"ab"}}"#)
                .unwrap();
        let DeviceMessage::Hello(h) = hello else {
            panic!("not a hello")
        };
        assert_eq!(h.protocol, 4);
    }

    #[test]
    fn null_reads_as_the_default() {
        let c = hello_control(r#"{"id":"iso","kind":"range","readonly":null,"min":null}"#);
        assert!(!c.readonly);
        assert_eq!(c.min, None);
        assert!(c.is_enabled(&Values::new()));
        let c = clock(r#""synced":null,"stratum":null,"freq_ppm":null,"update_age_s":null"#);
        assert!(!c.synced);
        assert_eq!(c.stratum, 0);
        assert_eq!(c.freq_ppm, 0.0);
        assert_eq!(c.update_age_s, None);
        let msg: DeviceMessage =
            serde_json::from_str(r#"{"type":"status","battery":{"level":null,"charging":null}}"#)
                .unwrap();
        let DeviceMessage::Status(s) = msg else {
            panic!("not a status")
        };
        assert_eq!(
            s.battery,
            Some(Battery {
                level: 0.0,
                charging: false
            })
        );
        // A required field must still be there.
        assert!(
            serde_json::from_str::<DeviceMessage>(r#"{"type":"status","battery":{}}"#).is_err()
        );
    }

    #[test]
    fn lenient_fields_serialize_unchanged() {
        let text = r#"{"type":"status","stream":{"state":"live","srt_latency_ms":120,"rtt_ms":2.5},
            "clock":{"synced":true,"stratum":3,"max_error_us":40,"sources":[{"reach":255,"offset_us":-7}]}}"#;
        let msg: DeviceMessage = serde_json::from_str(text).unwrap();
        let back = serde_json::to_value(&msg).unwrap();
        assert_eq!(back["stream"]["srt_latency_ms"], 120);
        assert_eq!(back["stream"]["rtt_ms"], 2.5);
        assert_eq!(back["clock"]["stratum"], 3);
        assert_eq!(back["clock"]["sources"][0]["offset_us"], -7);
        assert_eq!(serde_json::from_value::<DeviceMessage>(back).unwrap(), msg);
    }
}
