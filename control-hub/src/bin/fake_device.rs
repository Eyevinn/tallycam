//! Fake LCP v0 phone for testing the control hub without an iPhone.
//!
//! Usage: fake_device [--url ws://127.0.0.1:8090/lcp/v0/device] [--key PATH | --legacy-id ID]
//! [--name NAME] [--protocol 1|2|3] [--operator SECS]
//!
//! Proves a device key like the app: the hello carries the public key, the
//! hub's `challenge` is answered with a signed `auth`, and the device id is
//! derived from the key. `--key` keeps the key in a file (created when
//! missing), so the device is the same across runs; without it every run is
//! a new device. `--legacy-id` acts as an app before device keys (no key,
//! that id). Then sends a hello with the iOS prototype controls, applies `set` (clamping,
//! honouring `enabled_when`, replying with `result`), sends `status` every
//! second and generated JPEG previews while the hub asks for them. Reconnects
//! after a disconnect.
//!
//! With protocol 2 and up (3 is the default for a keyed device) it also acts
//! like a phone with shading (docs/SHADING.md): it applies the hub's
//! `production` (when the stream stops, if live) and sends `image_stats` from
//! its test picture while the hub asks for them. `--protocol 2` acts as the
//! 0.2.0 apps, which refuse exposure, colour and focus from the hub (they
//! wait for an `own` that a protocol 3 hub never sends); `--protocol 1` acts
//! as an app before shading.
//!
//! Mains works as on the phones: the production's `mains_hz` applies, a
//! `set` of `mains_hz` (from the hub or the operator) overrides it for this
//! camera until the production changes again, and `state.mains_hz` is the
//! one in use. With `--readonly-shutter` the shutter is `readonly` unless it
//! is free, as on the phones (the e2e tests set it freely). Resolution,
//! frame rate, audio codec and mains are `locked_while_live`.
//!
//! `--operator SECS` plays a camera operator: every SECS seconds it changes
//! exposure, colour, focus or mains on the "phone" (a `state` delta, as when
//! the operator turns a rocker), so the hub page can be checked to follow.

use std::f64::consts::PI;
use std::io::Cursor;
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Map, Value};
use tallycam_core::clock::report::clock_status;
use tallycam_core::clock::sim::{Scenario, SimServer, Simulation};
use tallycam_core::image_stats::{self, Grid};
use tallycam_hub::device_key::{self, SoftwareKey};
use tallycam_hub::protocol::{
    Battery, CommandDescriptor, ControlDescriptor, ControlKind, DeviceInfo, DeviceMessage,
    EnumOption, Hello, Log, Preview, Production, Scale, ServerToDevice, ShutterPolicy, Status,
    StreamStatus, TallyState, Values, CONTROL_PATH, DEVICE_PATH, PROTOCOL_VERSION,
};
use tallycam_hub::protocol::{ClockStatus, CommandResult, ImageStats, Percentiles, StreamLatency};
use tokio::time::{interval, sleep, sleep_until, MissedTickBehavior};
use tokio_tungstenite::tungstenite::Message;

/// A plausible `status.clock`: the core's clock discipline running against a
/// simulated network of eight NTP servers (Strom's pools), in step with the
/// fake device's uptime, so the hub shows iburst, sync and source states.
struct FakeClock {
    sim: Simulation,
}

impl FakeClock {
    fn new() -> Self {
        let pools = [0, 0, 0, 0, 1, 2, 3, 3];
        let servers = pools
            .iter()
            .enumerate()
            .map(|(i, &pool)| SimServer {
                pool,
                offset_ms: [0.1, -0.15, 0.05, 0.2, -0.1, 0.0, 0.12, -0.05][i],
                up_ms: 6.0 + 2.0 * i as f64,
                down_ms: 6.0 + 2.0 * i as f64,
                jitter_ms: 0.4 + 0.3 * i as f64,
                ..Default::default()
            })
            .collect();
        let scenario = Scenario {
            name: "fake_device".into(),
            servers,
            host_freq_ppm: 14.0,
            wall_error_ms: 40.0,
            duration_s: f64::MAX,
            ..Default::default()
        };
        FakeClock {
            sim: Simulation::new(scenario),
        }
    }

    fn status(&mut self, uptime_s: f64) -> ClockStatus {
        self.sim.run_until(uptime_s);
        let now = self.sim.host_now();
        let t = self.sim.engine.tracking_report(now);
        let sources = self.sim.engine.sources_report(now);
        clock_status(&t, &sources, Some(40.0))
    }
}

const SHUTTERS: [u64; 9] = [25, 30, 50, 60, 100, 120, 250, 500, 1000];

struct Args {
    url: String,
    key: Option<String>,
    legacy_id: Option<String>,
    name: String,
    /// `hello.protocol` of a keyed device (a legacy one always sends 0).
    protocol: u32,
    /// Seconds between simulated operator changes, if any.
    operator: Option<f64>,
    /// The shutter is `readonly` unless free, as on the phones.
    readonly_shutter: bool,
}

fn parse_args() -> Args {
    let mut a = Args {
        url: format!("ws://127.0.0.1:8090{DEVICE_PATH}"),
        key: None,
        legacy_id: None,
        name: "Fake iPhone".into(),
        protocol: PROTOCOL_VERSION,
        operator: None,
        readonly_shutter: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || {
            it.next()
                .unwrap_or_else(|| die(&format!("{arg} needs a value")))
        };
        match arg.as_str() {
            "--url" => a.url = val(),
            "--key" => a.key = Some(val()),
            "--legacy-id" => a.legacy_id = Some(val()),
            "--name" => a.name = val(),
            "--protocol" => {
                a.protocol = match val().parse() {
                    Ok(p) if (1..=PROTOCOL_VERSION).contains(&p) => p,
                    _ => die(&format!("--protocol must be 1 to {PROTOCOL_VERSION}")),
                }
            }
            "--operator" => {
                a.operator = match val().parse::<f64>() {
                    Ok(s) if s >= 0.5 => Some(s),
                    _ => die("--operator needs seconds, 0.5 or more"),
                }
            }
            "--readonly-shutter" => a.readonly_shutter = true,
            "--help" | "-h" => {
                println!(
                    "fake_device [--url URL] [--key PATH | --legacy-id ID] [--name NAME] [--protocol N]\n\
                     \x20           [--operator SECS] [--readonly-shutter]\n\
                     URL defaults to ws://127.0.0.1:8090{DEVICE_PATH} (controllers use {CONTROL_PATH}).\n\
                     --key PATH       device key file (hex P-256 secret), created when missing;\n\
                     \x20                default: a new key, and so a new device, on every run.\n\
                     --legacy-id ID   no device key, this id (an app before device keys).\n\
                     --protocol N     LCP version in hello, 1 to {PROTOCOL_VERSION} (default\n\
                     \x20                {PROTOCOL_VERSION}); 1 has no production or image stats, 2 (the\n\
                     \x20                0.2.0 apps) refuses exposure, colour and focus from the hub.\n\
                     --operator SECS  every SECS seconds the \"operator\" changes exposure,\n\
                     \x20                colour, focus or mains on the phone.\n\
                     --readonly-shutter  the shutter is read-only unless it is free, as on\n\
                     \x20                the phones (off by default: the e2e tests set it)."
                );
                std::process::exit(0);
            }
            _ => die(&format!("unknown argument '{arg}'")),
        }
    }
    if a.key.is_some() && a.legacy_id.is_some() {
        die("--key and --legacy-id exclude each other");
    }
    a
}

fn die(msg: &str) -> ! {
    eprintln!("fake_device: {msg}");
    std::process::exit(2)
}

// ---------------------------------------------------------------------------
// Descriptors
// ---------------------------------------------------------------------------

fn control(id: &str, label: &str, group: &str, kind: ControlKind) -> ControlDescriptor {
    ControlDescriptor {
        id: id.into(),
        label: Some(label.into()),
        group: Some(group.into()),
        kind,
        min: None,
        max: None,
        step: None,
        scale: None,
        unit: None,
        options: None,
        enabled_when: None,
        readonly: false,
        readonly_reason: None,
        extra: Map::new(),
    }
}

fn range(c: ControlDescriptor, min: f64, max: f64, step: f64, unit: &str) -> ControlDescriptor {
    ControlDescriptor {
        min: Some(min),
        max: Some(max),
        step: Some(step),
        unit: Some(unit.into()),
        ..c
    }
}

fn options(c: ControlDescriptor, opts: &[(Value, &str)]) -> ControlDescriptor {
    ControlDescriptor {
        options: Some(
            opts.iter()
                .map(|(v, l)| EnumOption {
                    value: v.clone(),
                    label: Some((*l).into()),
                })
                .collect(),
        ),
        ..c
    }
}

fn when(c: ControlDescriptor, key: &str, value: &str) -> ControlDescriptor {
    let mut m = Map::new();
    m.insert(key.into(), json!(value));
    ControlDescriptor {
        enabled_when: Some(m),
        ..c
    }
}

/// Refused while the stream is live (`locked_while_live`, PROTOCOL.md).
fn live_locked(mut c: ControlDescriptor) -> ControlDescriptor {
    c.extra.insert("locked_while_live".into(), json!(true));
    c
}

fn is_live_locked(c: &ControlDescriptor) -> bool {
    c.extra.get("locked_while_live") == Some(&json!(true))
}

/// The controls a protocol 2 phone (0.2.0) refuses from a hub that does not
/// own their group, which a protocol 3 hub never does.
const SHADING_CONTROLS: [&str; 9] = [
    "exposure_mode",
    "iso",
    "shutter",
    "ev_bias",
    "wb_mode",
    "wb_temperature",
    "wb_tint",
    "focus_mode",
    "focus_position",
];

fn log_scale(c: ControlDescriptor) -> ControlDescriptor {
    ControlDescriptor {
        scale: Some(Scale::Log),
        ..c
    }
}

/// Per-lens hardware limits, like an iPhone 16 Pro reports them.
struct LensCaps {
    zoom_max: f64,
    iso_min: f64,
    iso_max: f64,
    torch: bool,
}

fn lens_caps(lens: &str) -> LensCaps {
    match lens {
        "ultra_wide" => LensCaps {
            zoom_max: 4.0,
            iso_min: 40.0,
            iso_max: 2560.0,
            torch: true,
        },
        "tele" => LensCaps {
            zoom_max: 3.0,
            iso_min: 64.0,
            iso_max: 3200.0,
            torch: false,
        },
        _ => LensCaps {
            zoom_max: 8.0,
            iso_min: 32.0,
            iso_max: 2176.0,
            torch: true,
        },
    }
}

fn mode_options() -> Vec<(Value, &'static str)> {
    vec![(json!("auto"), "Auto"), (json!("manual"), "Manual")]
}

/// The descriptors for `lens`; the shutter is `readonly` unless it is free,
/// as on the phones.
fn controls_for(lens: &str, shutter_free: bool) -> Vec<ControlDescriptor> {
    use ControlKind::*;
    let caps = lens_caps(lens);
    let shutter_labels: Vec<(Value, String)> = SHUTTERS
        .iter()
        .map(|s| (json!(s), format!("1/{s}")))
        .collect();
    let mut v = vec![
        control("live", "Live", "stream", Bool),
        control("stream_url", "SRT URL", "stream", Text),
        range(
            control("bitrate_kbps", "Bitrate", "stream", Range),
            500.0,
            20000.0,
            100.0,
            "kbps",
        ),
        live_locked(options(
            control("audio_codec", "Audio codec", "stream", Enum),
            &[(json!("opus"), "Opus"), (json!("aac"), "AAC")],
        )),
        live_locked(options(
            control("resolution", "Resolution", "image", Enum),
            &[
                (json!("720p"), "720p"),
                (json!("1080p"), "1080p"),
                (json!("2160p"), "4K"),
            ],
        )),
        live_locked(options(
            control("frame_rate", "Frame rate", "image", Enum),
            &[
                (json!(25), "25 fps"),
                (json!(30), "30 fps"),
                (json!(50), "50 fps"),
                (json!(60), "60 fps"),
            ],
        )),
        options(
            control("lens", "Lens", "lens", Enum),
            &[
                (json!("ultra_wide"), "0.5x"),
                (json!("wide"), "1x"),
                (json!("tele"), "5x"),
            ],
        ),
        log_scale(range(
            control("zoom", "Zoom", "lens", Range),
            1.0,
            caps.zoom_max,
            0.01,
            "x",
        )),
        options(
            control("stabilization", "Stabilization", "image", Enum),
            &[
                (json!("off"), "Off"),
                (json!("standard"), "Standard"),
                (json!("cinematic"), "Cinematic"),
                (json!("cinematic_extended"), "Cinematic ext."),
            ],
        ),
        options(
            control("exposure_mode", "Exposure", "exposure", Enum),
            &mode_options(),
        ),
        log_scale(when(
            range(
                control("iso", "ISO", "exposure", Range),
                caps.iso_min,
                caps.iso_max,
                1.0,
                "",
            ),
            "exposure_mode",
            "manual",
        )),
        ControlDescriptor {
            readonly: !shutter_free,
            ..when(
                options(
                    control("shutter", "Shutter", "exposure", Enum),
                    &shutter_labels
                        .iter()
                        .map(|(v, l)| (v.clone(), l.as_str()))
                        .collect::<Vec<_>>(),
                ),
                "exposure_mode",
                "manual",
            )
        },
        when(
            range(
                control("ev_bias", "Exposure compensation", "exposure", Range),
                -3.0,
                3.0,
                0.1,
                "EV",
            ),
            "exposure_mode",
            "auto",
        ),
        live_locked(options(
            control("mains_hz", "Mains frequency", "exposure", Enum),
            &[
                (json!(50), "50 Hz"),
                (json!(60), "60 Hz"),
                (json!(0), "Off"),
            ],
        )),
        options(
            control("wb_mode", "White balance", "white_balance", Enum),
            &mode_options(),
        ),
        when(
            range(
                control("wb_temperature", "Temperature", "white_balance", Range),
                2500.0,
                10000.0,
                50.0,
                "K",
            ),
            "wb_mode",
            "manual",
        ),
        when(
            range(
                control("wb_tint", "Tint", "white_balance", Range),
                -150.0,
                150.0,
                1.0,
                "",
            ),
            "wb_mode",
            "manual",
        ),
        options(
            control("focus_mode", "Focus", "focus", Enum),
            &mode_options(),
        ),
        when(
            range(
                control("focus_position", "Focus position", "focus", Range),
                0.0,
                1.0,
                0.01,
                "",
            ),
            "focus_mode",
            "manual",
        ),
    ];
    if caps.torch {
        v.insert(7, control("torch", "Torch", "lens", Bool));
    }
    v
}

fn commands() -> Vec<CommandDescriptor> {
    let mut args = Map::new();
    args.insert("x".into(), json!("0..1"));
    args.insert("y".into(), json!("0..1"));
    vec![
        CommandDescriptor {
            name: "focus_at".into(),
            label: Some("Focus at point".into()),
            args: Some(args),
            extra: Map::new(),
        },
        CommandDescriptor {
            name: "wb_one_shot".into(),
            label: Some("One-shot white balance".into()),
            args: None,
            extra: Map::new(),
        },
    ]
}

// ---------------------------------------------------------------------------
// Simulated camera
// ---------------------------------------------------------------------------

struct Camera {
    info: DeviceInfo,
    /// None for `--legacy-id`.
    key: Option<SoftwareKey>,
    controls: Vec<ControlDescriptor>,
    state: Values,
    tally: TallyState,
    preview: Option<(f64, u32)>,
    focus_point: (f64, f64),
    battery: f64,
    live_since: Option<Instant>,
    started: Instant,
    rng: u64,
    clock: FakeClock,
    /// `hello.protocol`.
    protocol: u32,
    /// From the hub (protocol 2); `None` until it sends one.
    production: Option<Production>,
    /// `image_stats` per second while the hub asks for them.
    stats: Option<f64>,
    /// The production shutter in use (1/x s), `None` when the shutter is
    /// free. A new production waits until the stream stops.
    applied_shutter: Option<i64>,
    /// The phone's own mains setting (its Settings), used without a production.
    local_mains: u32,
    /// A `set` of `mains_hz` (hub or operator): wins over the production
    /// until the production changes again.
    mains_override: Option<u32>,
    /// The mains in use (`state.mains_hz`); held while live.
    applied_mains: u32,
    /// Simulated operator changes: the next step and when.
    operator: Option<(f64, usize, Instant)>,
    /// `--readonly-shutter`.
    readonly_shutter: bool,
}

/// The test picture's parameters at one moment, shared by the preview and
/// the image statistics.
struct Scene {
    exposure: f64,
    warm: f64,
    tint: f64,
    zoom: f64,
    torch: bool,
    t: f64,
}

impl Scene {
    /// Linear RGB (may exceed 1) at `xf`, `yf` in 0..1 of the frame.
    fn rgb(&self, xf: f64, yf: f64) -> [f64; 3] {
        let u = (xf - 0.5) / self.zoom * 4.0;
        let v = (yf - 0.5) / self.zoom * 4.0 * 9.0 / 16.0;
        let checker = (((u * 4.0).floor() + (v * 4.0).floor()) as i64).rem_euclid(2) as f64;
        let rings = 0.5 + 0.5 * (((u * u + v * v).sqrt() * 6.0 * PI) - self.t).sin();
        let mut base = 0.25 + 0.25 * checker + 0.3 * rings;
        if self.torch {
            let d = ((xf - 0.5).powi(2) + (yf - 0.5).powi(2)).sqrt();
            base += (0.4 - d).max(0.0);
        }
        let l = base * self.exposure;
        [
            l * (1.0 + 0.35 * self.warm),
            l * (1.0 - 0.25 * self.tint),
            l * (1.0 - 0.35 * self.warm),
        ]
    }
}

impl Camera {
    fn new(args: &Args) -> Self {
        let key = match (&args.key, &args.legacy_id) {
            (_, Some(_)) => None,
            (Some(path), None) => Some(
                device_key::load_or_create(std::path::Path::new(path)).unwrap_or_else(|e| die(&e)),
            ),
            (None, None) => Some(device_key::generate()),
        };
        let id = match (&key, &args.legacy_id) {
            (Some(key), _) => key.public_key().device_id(),
            (None, Some(id)) => id.clone(),
            (None, None) => unreachable!(),
        };
        let state = json!({
            "live": false,
            "stream_url": "srt://192.0.2.10:9000?streamid=cam1&latency=500",
            "bitrate_kbps": 6000,
            "resolution": "1080p",
            "frame_rate": 30,
            "audio_codec": "opus",
            "mains_hz": 50,
            "lens": "wide",
            "zoom": 1.0,
            "torch": false,
            "stabilization": "standard",
            "exposure_mode": "auto",
            "iso": 125,
            "shutter": 50,
            "ev_bias": 0.0,
            "wb_mode": "auto",
            "wb_temperature": 5200,
            "wb_tint": 0,
            "focus_mode": "auto",
            "focus_position": 0.6
        });
        Self {
            key,
            info: DeviceInfo {
                id,
                name: args.name.clone(),
                model: "iPhone17,1".into(),
                platform: "ios".into(),
                os_version: "26.0".into(),
                app_version: "0.1.0-fake".into(),
                extra: Map::new(),
            },
            controls: controls_for("wide", !args.readonly_shutter),
            state: state.as_object().cloned().unwrap_or_default(),
            tally: TallyState::Off,
            preview: None,
            focus_point: (0.5, 0.5),
            battery: 0.87,
            live_since: None,
            started: Instant::now(),
            rng: 0x2545_f491_4f6c_dd1d ^ u64::from(std::process::id()),
            clock: FakeClock::new(),
            protocol: if args.legacy_id.is_some() {
                0
            } else {
                args.protocol
            },
            production: None,
            stats: None,
            applied_shutter: Some(50),
            local_mains: 50,
            mains_override: None,
            applied_mains: 50,
            readonly_shutter: args.readonly_shutter,
            operator: args
                .operator
                .map(|secs| (secs, 0, Instant::now() + Duration::from_secs_f64(secs))),
        }
    }

    fn shading(&self) -> bool {
        self.protocol >= 2
    }

    fn rand(&mut self) -> f64 {
        // xorshift64*
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn hello(&self) -> DeviceMessage {
        DeviceMessage::Hello(Hello {
            protocol: self.protocol,
            device: self.info.clone(),
            public_key: self.key.as_ref().map(|k| k.public_key().to_base64()),
            controls: self.controls.clone(),
            commands: commands(),
        })
    }

    fn full_state(&self) -> DeviceMessage {
        let mut values = self.state.clone();
        let present: Vec<&str> = self.controls.iter().map(|c| c.id.as_str()).collect();
        values.retain(|k, _| present.contains(&k.as_str()));
        DeviceMessage::State { values }
    }

    fn str_of(&self, key: &str) -> &str {
        self.state.get(key).and_then(Value::as_str).unwrap_or("")
    }

    fn num_of(&self, key: &str) -> f64 {
        self.state.get(key).and_then(Value::as_f64).unwrap_or(0.0)
    }

    /// Validate and apply one value. Returns the error text, if any; a clamped
    /// value is applied and still reported as an error.
    fn apply_one(&mut self, key: &str, value: &Value) -> Option<String> {
        let Some(desc) = self.controls.iter().find(|c| c.id == key).cloned() else {
            return Some(format!("unknown control '{key}'"));
        };
        if desc.readonly {
            return Some(format!("{key} is read-only"));
        }
        if !desc.is_enabled(&self.state) {
            let cond = desc
                .enabled_when
                .as_ref()
                .map(|m| {
                    m.iter()
                        .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or(&v.to_string())))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            return Some(format!("{key} is only writable when {cond}"));
        }
        if is_live_locked(&desc)
            && self.is_live()
            && !self.state.get(key).is_some_and(|v| json_eq(v, value))
        {
            return Some(format!(
                "{key}: not while live, stop the stream to change it"
            ));
        }
        if key == "mains_hz" {
            // As on the phones: wins over the production until it changes.
            return match value.as_u64().filter(|hz| [0, 50, 60].contains(hz)) {
                Some(hz) => {
                    self.mains_override = Some(hz as u32);
                    self.local_mains = hz as u32;
                    self.state.insert(key.into(), json!(hz));
                    None
                }
                None => Some("mains_hz: expected 50, 60 or 0 (off)".into()),
            };
        }
        match desc.kind {
            ControlKind::Range => {
                let Some(mut x) = value.as_f64() else {
                    return Some(format!("{key} needs a number"));
                };
                let (min, max) = (desc.min.unwrap_or(f64::MIN), desc.max.unwrap_or(f64::MAX));
                let mut err = None;
                if x < min || x > max {
                    let c = x.clamp(min, max);
                    err = Some(format!(
                        "{key} {x} {} {} {}, clamped",
                        if x < min { "below" } else { "above" },
                        if x < min { "min" } else { "max" },
                        c
                    ));
                    x = c;
                }
                if let Some(step) = desc.step.filter(|s| *s > 0.0) {
                    x = ((x - min) / step).round() * step + min;
                    x = (x * 1e6).round() / 1e6;
                }
                let v = if desc.step.is_some_and(|s| s.fract() == 0.0) && x.fract() == 0.0 {
                    json!(x as i64)
                } else {
                    json!(x)
                };
                self.state.insert(key.into(), v);
                err
            }
            ControlKind::Enum => {
                let opts = desc.options.unwrap_or_default();
                let found = opts.iter().find(|o| json_eq(&o.value, value));
                match found {
                    Some(o) => {
                        self.state.insert(key.into(), o.value.clone());
                        None
                    }
                    None => Some(format!("{key}: {value} is not a supported option")),
                }
            }
            ControlKind::Bool => match value.as_bool() {
                Some(b) => {
                    self.state.insert(key.into(), json!(b));
                    None
                }
                None => Some(format!("{key} needs true or false")),
            },
            ControlKind::Text => match value.as_str() {
                Some(s) => {
                    self.state.insert(key.into(), json!(s));
                    None
                }
                None => Some(format!("{key} needs a string")),
            },
            ControlKind::Unknown => Some(format!("{key}: unsupported control kind")),
        }
    }

    /// The refusal of a protocol 2 phone (0.2.0) for a shading control or
    /// command from the hub: it never owns the group under protocol 3.
    fn refuses(&self, key: &str) -> Option<String> {
        if self.protocol != 2 {
            return None;
        }
        let group = match key {
            "focus_at" => "focus",
            "wb_one_shot" => "white balance",
            k if !SHADING_CONTROLS.contains(&k) => return None,
            k if k.starts_with("wb_") => "white balance",
            k if k.starts_with("focus_") => "focus",
            _ => "exposure",
        };
        Some(format!("{key}: {group} is not owned by this server"))
    }

    /// Apply a `set` from the hub (`from_hub`) or the operator. Returns the
    /// messages to send back.
    fn apply_set(
        &mut self,
        id: Option<Value>,
        values: Values,
        from_hub: bool,
    ) -> Vec<DeviceMessage> {
        let mut errors = Vec::new();
        let values: Values = values
            .into_iter()
            // "locked" from an older hub page or app: manual where it is.
            .map(|(k, v)| match (k.ends_with("_mode"), v.as_str()) {
                (true, Some("locked")) => (k, json!("manual")),
                _ => (k, v),
            })
            .filter(|(k, _)| match self.refuses(k).filter(|_| from_hub) {
                Some(why) => {
                    errors.push(why);
                    false
                }
                None => true,
            })
            .collect();
        let old_lens = self.str_of("lens").to_string();
        // Controls without a condition first, so `{"exposure_mode":"manual",
        // "iso":400}` in one set works.
        let (plain, conditional): (Vec<_>, Vec<_>) = values.iter().partition(|(k, _)| {
            self.controls
                .iter()
                .find(|c| &c.id == *k)
                .is_none_or(|c| c.enabled_when.is_none())
        });
        for (k, v) in plain.into_iter().chain(conditional) {
            if let Some(e) = self.apply_one(k, v) {
                errors.push(e);
            }
        }
        let mut out = Vec::new();
        let mut reported: Values = Values::new();
        if self.str_of("lens") != old_lens {
            // New lens: new ranges, resend hello and the full state.
            let lens = self.str_of("lens").to_string();
            self.controls = controls_for(&lens, self.shutter_writable());
            let caps = lens_caps(&lens);
            let zoom = self.num_of("zoom").clamp(1.0, caps.zoom_max);
            self.state.insert("zoom".into(), json!(zoom));
            let iso = self.num_of("iso").clamp(caps.iso_min, caps.iso_max);
            self.state.insert("iso".into(), json!(iso.round() as i64));
            if !caps.torch {
                self.state.insert("torch".into(), json!(false));
            }
            out.push(self.hello());
            out.push(self.full_state());
        } else {
            // Report the real value of every key asked for, so a UI that
            // moved a slider snaps back to what the camera did.
            for k in values.keys() {
                if let Some(v) = self.state.get(k) {
                    reported.insert(k.clone(), v.clone());
                }
            }
            if values.contains_key("live") {
                self.live_since = self.state["live"]
                    .as_bool()
                    .unwrap_or(false)
                    .then(Instant::now);
            }
            // A new mains (or a stopped stream) moves the shutter now.
            let (changed, reshaped) = self.settle();
            reported.extend(changed);
            out.push(DeviceMessage::State { values: reported });
            if reshaped {
                out.push(self.hello());
                out.push(self.full_state());
            }
        }
        let error = (!errors.is_empty()).then(|| errors.join("; "));
        match id {
            Some(id) => out.push(DeviceMessage::Result(CommandResult {
                id,
                ok: error.is_none(),
                error,
                data: None,
            })),
            None => {
                if let Some(message) = error {
                    out.push(DeviceMessage::Log(Log {
                        level: "warn".into(),
                        message,
                    }))
                }
            }
        }
        out
    }

    fn apply_command(
        &mut self,
        id: Option<Value>,
        name: &str,
        args: Option<Map<String, Value>>,
    ) -> Vec<DeviceMessage> {
        let mut out = Vec::new();
        let refused = self.refuses(name);
        let res: Result<(), String> = match name {
            _ if refused.is_some() => Err(refused.unwrap_or_default()),
            "focus_at" => {
                let args = args.unwrap_or_default();
                let x = args.get("x").and_then(Value::as_f64);
                let y = args.get("y").and_then(Value::as_f64);
                match (x, y) {
                    (Some(x), Some(y)) if (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y) => {
                        self.focus_point = (x, y);
                        let mut values = Values::new();
                        if self.str_of("focus_mode") != "auto" {
                            self.state.insert("focus_mode".into(), json!("auto"));
                            values.insert("focus_mode".into(), json!("auto"));
                        }
                        let pos = ((1.0 - y) * 1000.0).round() / 1000.0;
                        self.state.insert("focus_position".into(), json!(pos));
                        values.insert("focus_position".into(), json!(pos));
                        out.push(DeviceMessage::State { values });
                        out.push(DeviceMessage::Log(Log {
                            level: "info".into(),
                            message: format!("Focus and exposure point set to {x:.2}, {y:.2}"),
                        }));
                        Ok(())
                    }
                    _ => Err("focus_at needs x and y in 0..1".into()),
                }
            }
            "wb_one_shot" => {
                let k = (5600.0 + (self.rand() - 0.5) * 400.0) / 50.0;
                let k = (k.round() * 50.0) as i64;
                // As the phones: measure, then manual at what was measured.
                self.state.insert("wb_mode".into(), json!("manual"));
                self.state.insert("wb_temperature".into(), json!(k));
                let mut values = Values::new();
                values.insert("wb_mode".into(), json!("manual"));
                values.insert("wb_temperature".into(), json!(k));
                out.push(DeviceMessage::State { values });
                Ok(())
            }
            _ => Err(format!("unknown command '{name}'")),
        };
        if let Some(id) = id {
            out.push(DeviceMessage::Result(CommandResult {
                id,
                ok: res.is_ok(),
                error: res.err(),
                data: None,
            }));
        }
        out
    }

    fn is_live(&self) -> bool {
        self.state
            .get("live")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Is the shutter writable (in manual exposure)? When it is free, or
    /// always without `--readonly-shutter`.
    fn shutter_writable(&self) -> bool {
        !self.readonly_shutter || self.applied_shutter.is_none()
    }

    /// The mains frequency that applies now: the camera's own override, else
    /// the production's, else the phone's own setting.
    fn effective_mains(&self) -> u32 {
        self.mains_override
            .or(self.production.as_ref().map(|p| p.mains_hz))
            .unwrap_or(self.local_mains)
    }

    /// The shutter the production asks for at the mains in use, made to fit
    /// the frame rate (a shutter longer than a frame doubles to the next
    /// flicker-free value); `None` when the shutter is free.
    fn production_shutter(&self) -> Option<i64> {
        let policy = self
            .production
            .as_ref()
            .map(|p| p.shutter)
            .unwrap_or_default();
        let mains = self.effective_mains();
        let mut shutter = match policy {
            ShutterPolicy::Free => return None,
            ShutterPolicy::Mains if mains == 0 => return None,
            ShutterPolicy::Mains if mains == 60 => 60,
            ShutterPolicy::Fixed(x) => i64::from(x),
            ShutterPolicy::Mains => 50,
        };
        let fps = self.num_of("frame_rate").max(1.0) as i64;
        while shutter < fps {
            shutter *= 2;
        }
        Some(shutter)
    }

    /// `production` from the hub; the shutter applies on the next tick, or
    /// when the stream stops.
    fn apply_production(&mut self, production: Production) -> Vec<DeviceMessage> {
        println!(
            "fake_device: production mains {} Hz, shutter {:?}",
            production.mains_hz, production.shutter
        );
        self.production = Some(production);
        // A new production wins over this camera's own mains again.
        self.mains_override = None;
        if self.is_live()
            && (self.production_shutter() != self.applied_shutter
                || self.effective_mains() != self.applied_mains)
        {
            return vec![DeviceMessage::Log(Log {
                level: "warn".into(),
                message: "New production applies when the stream stops".into(),
            })];
        }
        let (values, reshaped) = self.settle();
        let mut out = Vec::new();
        if !values.is_empty() {
            out.push(DeviceMessage::State { values });
        }
        if reshaped {
            out.push(self.hello());
            out.push(self.full_state());
        }
        out
    }

    /// Unless live, applies the mains and production shutter now. Returns
    /// the changed values and whether the descriptors changed (the shutter
    /// became free or fixed: a new `hello`).
    fn settle(&mut self) -> (Values, bool) {
        let mut changed = Values::new();
        if self.is_live() {
            return (changed, false);
        }
        self.applied_mains = self.effective_mains();
        let shutter = self.production_shutter();
        let moved = shutter != self.applied_shutter;
        self.applied_shutter = shutter;
        let mains = json!(self.applied_mains);
        if self.state.get("mains_hz") != Some(&mains) {
            self.state.insert("mains_hz".into(), mains.clone());
            changed.insert("mains_hz".into(), mains);
        }
        // A manual shutter (without --readonly-shutter) stays until the
        // production's moves.
        let manual = self.str_of("exposure_mode") == "manual";
        if let Some(shutter) = self.applied_shutter.filter(|_| moved || !manual) {
            let v = json!(shutter);
            if self.state.get("shutter") != Some(&v) {
                self.state.insert("shutter".into(), v.clone());
                changed.insert("shutter".into(), v);
            }
        }
        let controls = controls_for(self.str_of("lens"), self.shutter_writable());
        let reshaped = controls != self.controls;
        self.controls = controls;
        (changed, reshaped)
    }

    /// The next simulated operator change (`--operator`), as a `set` from the
    /// phone itself.
    fn operator_step(&mut self) -> Vec<DeviceMessage> {
        let Some((secs, step, _)) = self.operator else {
            return Vec::new();
        };
        const STEPS: usize = 9;
        let values = match step % STEPS {
            0 => json!({ "wb_mode": "manual", "wb_temperature": 4300, "wb_tint": 6 }),
            1 => json!({ "exposure_mode": "manual", "iso": 400 }),
            2 => json!({ "focus_mode": "manual", "focus_position": 0.42 }),
            3 => json!({ "exposure_mode": "auto", "ev_bias": 0.7 }),
            4 => json!({ "mains_hz": 60 }),
            5 => json!({ "wb_mode": "auto" }),
            // A touch-and-hold focus lock: manual where autofocus was.
            6 => json!({ "focus_mode": "manual", "focus_position": 0.6 }),
            7 => json!({ "ev_bias": 0.0, "focus_mode": "auto" }),
            _ => json!({ "mains_hz": 50 }),
        };
        self.operator = Some((
            secs,
            step + 1,
            Instant::now() + Duration::from_secs_f64(secs),
        ));
        let values = values.as_object().cloned().unwrap_or_default();
        println!(
            "fake_device: operator sets {}",
            Value::Object(values.clone())
        );
        self.apply_set(None, values, false)
    }

    fn next_operator(&self, far: tokio::time::Instant) -> tokio::time::Instant {
        self.operator
            .map(|(_, _, at)| tokio::time::Instant::from_std(at))
            .unwrap_or(far)
    }

    /// Once a second: auto modes drift, so the operator sees measured values.
    /// Also returns whether the descriptors changed (a new `hello`).
    fn tick(&mut self) -> (Status, Values, bool) {
        let t = self.started.elapsed().as_secs_f64();
        // The mains and production shutter, held while live.
        let (mut changed, reshaped) = self.settle();
        if self.str_of("exposure_mode") == "auto" {
            let caps = lens_caps(self.str_of("lens"));
            let ev = self.num_of("ev_bias");
            let iso = (200.0 * 2f64.powf(ev) * (1.0 + 0.3 * (t / 7.0).sin()))
                .clamp(caps.iso_min, caps.iso_max)
                .round() as i64;
            let mut auto = vec![("iso", json!(iso))];
            if self.applied_shutter.is_none() {
                auto.push(("shutter", json!(if iso > 400 { 50 } else { 100 })));
            }
            for (k, v) in auto {
                if self.state.get(k) != Some(&v) {
                    self.state.insert(k.into(), v.clone());
                    changed.insert(k.into(), v);
                }
            }
        }
        if self.str_of("wb_mode") == "auto" {
            let k = ((5200.0 + 150.0 * (t / 11.0).sin()) / 50.0).round() as i64 * 50;
            let v = json!(k);
            if self.state.get("wb_temperature") != Some(&v) {
                self.state.insert("wb_temperature".into(), v.clone());
                changed.insert("wb_temperature".into(), v);
            }
        }
        let charging = false;
        self.battery = (self.battery - 0.0002).max(0.05);
        let live = self
            .state
            .get("live")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let url = self.str_of("stream_url").to_string();
        let stream = if live {
            let connecting = self
                .live_since
                .is_some_and(|s| s.elapsed() < Duration::from_millis(1500));
            let target = self.num_of("bitrate_kbps");
            StreamStatus {
                state: if connecting { "connecting" } else { "live" }.into(),
                url: Some(url),
                bitrate_kbps: (!connecting).then(|| (target * (0.95 + 0.08 * self.rand())).round()),
                rtt_ms: (!connecting).then(|| ((10.0 + 6.0 * self.rand()) * 10.0).round() / 10.0),
                retransmits: (!connecting).then_some((t as u64) / 13),
                dropped: (!connecting).then_some(0),
                error: None,
                srt_latency_ms: (!connecting).then_some(120),
                latency: (!connecting).then(|| {
                    let p = |p50: f64| Percentiles {
                        p50,
                        p95: (p50 * 1.3 * 10.0).round() / 10.0,
                        max: (p50 * 1.8 * 10.0).round() / 10.0,
                    };
                    Box::new(StreamLatency {
                        capture_to_encode_ms: Some(p(4.0)),
                        encode_ms: Some(p(9.0)),
                        mux_ms: Some(p(0.5)),
                        send_queue_ms: Some(p(0.3)),
                        send_frame_ms: Some(p(2.0)),
                        capture_to_send_ms: Some(p(14.0)),
                        audio_capture_to_encode_ms: Some(p(7.0)),
                        audio_encode_ms: Some(p(0.4)),
                        audio_capture_to_mux_ms: Some(p(25.0)),
                        extra: Map::new(),
                    })
                }),
                late_video_frames: (!connecting).then_some(0),
                late_audio_frames: (!connecting).then_some(0),
                extra: Map::new(),
            }
        } else {
            StreamStatus {
                state: "idle".into(),
                url: Some(url),
                bitrate_kbps: None,
                rtt_ms: None,
                retransmits: None,
                dropped: None,
                error: None,
                srt_latency_ms: None,
                latency: None,
                late_video_frames: None,
                late_audio_frames: None,
                extra: Map::new(),
            }
        };
        let thermal = if t > 600.0 { "fair" } else { "nominal" };
        let status = Status {
            battery: Some(Battery {
                level: (self.battery * 100.0).round() / 100.0,
                charging,
            }),
            thermal: Some(thermal.into()),
            stream: Some(stream),
            network: Some("wifi".into()),
            clock: Some(Box::new(self.clock.status(t))),
            extra: Map::new(),
        };
        (status, changed, reshaped)
    }

    fn scene(&self) -> Scene {
        let t = self.started.elapsed().as_secs_f64();
        // Exposure: brighter with higher ISO and longer shutter.
        let iso = self.num_of("iso").max(1.0);
        let shutter = self.num_of("shutter").max(1.0);
        let exposure = ((iso / 200.0) * (100.0 / shutter))
            .powf(0.5)
            .clamp(0.15, 2.5)
            * 2f64.powf(if self.str_of("exposure_mode") == "auto" {
                self.num_of("ev_bias") * 0.5
            } else {
                0.0
            });
        // White balance: low Kelvin setting -> image looks blue, high -> warm.
        let k = self.num_of("wb_temperature");
        let warm = ((k - 5200.0) / 4000.0).clamp(-1.0, 1.0);
        let tint = self.num_of("wb_tint") / 300.0;
        let zoom = self.num_of("zoom").max(1.0)
            * match self.str_of("lens") {
                "ultra_wide" => 0.5,
                "tele" => 5.0,
                _ => 1.0,
            };
        let torch = self
            .state
            .get("torch")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Scene {
            exposure,
            warm,
            tint,
            zoom,
            torch,
            t,
        }
    }

    /// `image_stats` of the test picture, on a coarse grid as a phone samples
    /// it, so the hub's shader view reacts to ISO and white balance.
    fn image_stats(&self) -> Option<ImageStats> {
        const W: usize = 64;
        const H: usize = 36;
        let scene = self.scene();
        let mut y = Vec::with_capacity(W * H);
        let mut cbcr = Vec::with_capacity(2 * W * H);
        for row in 0..H {
            for col in 0..W {
                let xf = (col as f64 + 0.5) / W as f64;
                let yf = (row as f64 + 0.5) / H as f64;
                let [r, g, b] = scene.rgb(xf, yf).map(|c| c.clamp(0.0, 1.0));
                // BT.709 Y'CbCr, video range.
                let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                let pb = (b - luma) / 1.8556;
                let pr = (r - luma) / 1.5748;
                y.push((16.0 + 219.0 * luma).round() as u8);
                cbcr.push((128.0 + 224.0 * pb).round().clamp(16.0, 240.0) as u8);
                cbcr.push((128.0 + 224.0 * pr).round().clamp(16.0, 240.0) as u8);
            }
        }
        let mut stats = image_stats::compute(&Grid {
            y: &y,
            cbcr: &cbcr,
            width: W,
            height: H,
            video_range: true,
        })?;
        let mut camera = Values::new();
        for k in ["iso", "shutter", "wb_temperature", "wb_tint"] {
            if let Some(v) = self.state.get(k) {
                camera.insert(k.into(), v.clone());
            }
        }
        let ev = (scene.exposure.log2() * 10.0).round() / 10.0;
        camera.insert("exposure_offset_ev".into(), json!(ev));
        stats.camera = Some(camera);
        Some(stats)
    }

    /// A generated test picture that reacts to the controls.
    fn render_preview(&self, width: u32) -> Option<Preview> {
        let width = width.clamp(64, 1280);
        let height = width * 9 / 16;
        let scene = self.scene();
        let t = scene.t;
        let (fx, fy) = self.focus_point;
        let tally_rgb = match self.tally {
            TallyState::Program => Some([220u8, 30, 30]),
            TallyState::Preview => Some([30, 200, 60]),
            TallyState::Off => None,
        };
        let bar_x = ((t * 0.2).fract() * width as f64) as u32;

        let mut img = image::RgbImage::new(width, height);
        for (x, y, px) in img.enumerate_pixels_mut() {
            let mut rgb = scene.rgb(x as f64 / width as f64, y as f64 / height as f64);
            if x.abs_diff(bar_x) < 3 {
                rgb = [0.9, 0.9, 0.2];
            }
            *px = image::Rgb(rgb.map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8));
            if let Some(c) = tally_rgb {
                if x < 6 || y < 6 || x >= width - 6 || y >= height - 6 {
                    *px = image::Rgb(c);
                }
            }
        }
        // Focus crosshair.
        let cx = (fx * width as f64) as i64;
        let cy = (fy * height as f64) as i64;
        for d in -12i64..=12 {
            for (px, py) in [(cx + d, cy), (cx, cy + d)] {
                if px >= 0 && py >= 0 && (px as u32) < width && (py as u32) < height {
                    img.put_pixel(px as u32, py as u32, image::Rgb([255, 210, 0]));
                }
            }
        }

        let mut buf = Cursor::new(Vec::new());
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 70);
        if let Err(e) = img.write_with_encoder(enc) {
            eprintln!("fake_device: JPEG encode failed: {e}");
            return None;
        }
        Some(Preview {
            jpeg: base64::engine::general_purpose::STANDARD.encode(buf.into_inner()),
            width,
            height,
        })
    }
}

fn json_eq(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

// ---------------------------------------------------------------------------
// Connection loop
// ---------------------------------------------------------------------------

fn text(msg: &DeviceMessage) -> Message {
    Message::Text(serde_json::to_string(msg).unwrap_or_default().into())
}

/// Applies one hub message; returns what to send back.
fn on_server_message(
    cam: &mut Camera,
    msg: ServerToDevice,
    next_preview: &mut tokio::time::Instant,
    next_stats: &mut tokio::time::Instant,
    far: tokio::time::Instant,
) -> Vec<DeviceMessage> {
    let mut out = Vec::new();
    match msg {
        ServerToDevice::Set { id, values } => {
            println!(
                "fake_device: set {}",
                serde_json::Value::Object(values.clone())
            );
            out = cam.apply_set(id, values, true)
        }
        ServerToDevice::Command { id, name, args } => out = cam.apply_command(id, &name, args),
        ServerToDevice::Tally { state } => {
            println!("fake_device: tally {state:?}");
            cam.tally = state;
        }
        ServerToDevice::Message { text } => {
            println!("fake_device: operator message: {text}");
            out.push(DeviceMessage::Log(Log {
                level: "info".into(),
                message: format!("Showed operator message: {text}"),
            }));
        }
        ServerToDevice::Preview {
            enabled,
            fps,
            width,
        } => {
            if enabled {
                let fps = fps.filter(|f| *f > 0.0).unwrap_or(1.0).min(30.0);
                cam.preview = Some((fps, width.unwrap_or(480)));
                *next_preview = tokio::time::Instant::now();
            } else {
                cam.preview = None;
                *next_preview = far;
            }
            println!("fake_device: preview {:?}", cam.preview);
        }
        // Protocol 1 apps do not know these (they parse as unknown and are
        // ignored), so neither does the fake device then.
        ServerToDevice::Production(_) | ServerToDevice::Stats { .. } if !cam.shading() => {}
        ServerToDevice::Production(p) => out = cam.apply_production(p),
        ServerToDevice::Stats { enabled, fps } => {
            if enabled {
                cam.stats = Some(fps.filter(|f| *f > 0.0).unwrap_or(1.0).clamp(0.2, 4.0));
                *next_stats = tokio::time::Instant::now();
            } else {
                cam.stats = None;
                *next_stats = far;
            }
            println!("fake_device: image stats {:?}", cam.stats);
        }
        ServerToDevice::Ping(p) => out.push(DeviceMessage::Pong(p)),
        // The fake device does not sync its clock; a late challenge is ignored.
        ServerToDevice::Pong(_) | ServerToDevice::Challenge { .. } => {}
    }
    out
}

async fn run_once(args: &Args, cam: &mut Camera) -> Result<(), String> {
    let (ws, _) = tokio_tungstenite::connect_async(args.url.as_str())
        .await
        .map_err(|e| format!("connect failed: {e}"))?;
    println!("fake_device: connected to {} as {}", args.url, cam.info.id);
    let (mut sink, mut stream) = ws.split();
    let send_err = |e: tokio_tungstenite::tungstenite::Error| format!("send failed: {e}");

    sink.send(text(&cam.hello())).await.map_err(send_err)?;
    // Admission: answer the hub's challenge, if it sends one, until the
    // first `tally` (sent once the hub admitted the device). A refusal is a
    // `message`, then the hub closes the socket.
    let mut early = Vec::new();
    loop {
        let frame = match stream.next().await {
            None => return Err("closed by hub before admission".into()),
            Some(Err(e)) => return Err(format!("read failed: {e}")),
            Some(Ok(f)) => f,
        };
        let Message::Text(t) = frame else {
            if let Message::Close(_) = frame {
                return Err("closed by hub before admission".into());
            }
            continue;
        };
        match serde_json::from_str::<ServerToDevice>(t.as_str()) {
            Ok(ServerToDevice::Challenge { nonce, server }) => {
                let Some(key) = &cam.key else {
                    println!("fake_device: challenge, but no device key (legacy)");
                    continue;
                };
                // Signed for the host it dialled, as the apps do.
                let host =
                    tallycam_hub::device_key::auth_host(args.url.as_str()).unwrap_or_default();
                let signature = key.sign_auth(&server, &nonce, &host);
                sink.send(text(&DeviceMessage::Auth { signature, host }))
                    .await
                    .map_err(send_err)?;
            }
            Ok(ServerToDevice::Message { text }) => {
                println!("fake_device: hub says: {text}");
            }
            Ok(msg @ ServerToDevice::Tally { .. }) => {
                println!("fake_device: admitted");
                early.push(msg);
                break;
            }
            Ok(msg) => early.push(msg),
            Err(_) => {} // unknown types are ignored
        }
    }
    sink.send(text(&cam.full_state())).await.map_err(send_err)?;
    cam.preview = None;
    cam.stats = None;

    let mut status_tick = interval(Duration::from_secs(1));
    status_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let far = tokio::time::Instant::now() + Duration::from_secs(86400 * 365);
    let mut next_preview = far;
    let mut next_stats = far;
    for msg in early {
        for m in on_server_message(cam, msg, &mut next_preview, &mut next_stats, far) {
            sink.send(text(&m)).await.map_err(send_err)?;
        }
    }

    loop {
        let mut out: Vec<DeviceMessage> = Vec::new();
        tokio::select! {
            frame = stream.next() => {
                let frame = match frame {
                    None => return Err("connection closed".into()),
                    Some(Err(e)) => return Err(format!("read failed: {e}")),
                    Some(Ok(f)) => f,
                };
                let Message::Text(t) = frame else {
                    if let Message::Close(_) = frame { return Err("closed by hub".into()); }
                    continue;
                };
                let msg: ServerToDevice = match serde_json::from_str(t.as_str()) {
                    Ok(m) => m,
                    Err(_) => continue, // unknown types are ignored
                };
                out = on_server_message(cam, msg, &mut next_preview, &mut next_stats, far);
            }
            _ = status_tick.tick() => {
                let (status, changed, reshaped) = cam.tick();
                if !changed.is_empty() {
                    out.push(DeviceMessage::State { values: changed });
                }
                if reshaped {
                    out.push(cam.hello());
                    out.push(cam.full_state());
                }
                out.push(DeviceMessage::Status(status));
            }
            _ = sleep_until(cam.next_operator(far)) => {
                out = cam.operator_step();
            }
            _ = sleep_until(next_preview) => {
                if let Some((fps, width)) = cam.preview {
                    if let Some(p) = cam.render_preview(width) {
                        out.push(DeviceMessage::Preview(p));
                    }
                    next_preview = tokio::time::Instant::now() + Duration::from_secs_f64(1.0 / fps);
                } else {
                    next_preview = far;
                }
            }
            _ = sleep_until(next_stats) => {
                if let Some(fps) = cam.stats {
                    if let Some(stats) = cam.image_stats() {
                        out.push(DeviceMessage::ImageStats(stats));
                    }
                    next_stats = tokio::time::Instant::now() + Duration::from_secs_f64(1.0 / fps);
                } else {
                    next_stats = far;
                }
            }
        }
        for m in &out {
            sink.send(text(m)).await.map_err(send_err)?;
        }
    }
}

#[tokio::main]
async fn main() {
    let args = parse_args();
    let mut cam = Camera::new(&args);
    loop {
        if let Err(e) = run_once(&args, &mut cam).await {
            eprintln!("fake_device: {e}, reconnecting in 2 s");
        }
        sleep(Duration::from_secs(2)).await;
    }
}
