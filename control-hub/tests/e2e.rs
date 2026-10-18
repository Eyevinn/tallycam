//! End-to-end: hub in-process, the real `fake_device` binary, a raw test
//! device and controller clients over WebSockets.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tallycam_hub::device_key::{self, SoftwareKey};
use tallycam_hub::protocol::CONTROL_PATH;
use tallycam_hub::Hub;
use tallycam_hub::Provisioning;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn start_hub() -> SocketAddr {
    start_hub_with(Provisioning::new).await
}

async fn start_hub_with(provisioning: impl FnOnce(SocketAddr) -> Provisioning) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = tallycam_hub::server::router(Arc::new(Hub::new()), provisioning(addr));
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    addr
}

/// A fresh directory for this test's files.
fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "tallycam-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn connect(addr: SocketAddr, path: &str) -> Ws {
    connect_async(format!("ws://{addr}{path}")).await.unwrap().0
}

async fn send(ws: &mut Ws, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

/// Read JSON frames until `pred` matches, failing after `secs`.
async fn wait_for(ws: &mut Ws, secs: u64, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
    let fut = async {
        loop {
            match ws.next().await {
                Some(Ok(Message::Text(t))) => {
                    let v: Value = serde_json::from_str(t.as_str()).unwrap();
                    if pred(&v) {
                        return v;
                    }
                }
                Some(Ok(_)) => {}
                other => panic!("socket ended while waiting for {what}: {other:?}"),
            }
        }
    };
    timeout(Duration::from_secs(secs), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
}

fn is(v: &Value, ty: &str, device: &str) -> bool {
    v["type"] == ty && v["device"] == device
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hub_end_to_end() {
    let addr = start_hub().await;

    // A raw device first: no controller yet, so previews are off.
    let mut raw = connect(addr, "/lcp/v0/device").await;
    send(
        &mut raw,
        json!({"type":"hello","protocol":0,
               "device":{"id":"raw-1","name":"Raw","model":"Test","future_field":1},
               "controls":[{"id":"gain","kind":"range","min":0,"max":10,"group":"audio"}],
               "commands":[]}),
    )
    .await;
    send(&mut raw, json!({"type":"state","values":{"gain":3}})).await;
    let p = wait_for(&mut raw, 3, "initial preview off", |v| {
        v["type"] == "preview"
    })
    .await;
    assert_eq!(p["enabled"], false, "no controller: preview must be off");

    // The real fake device, with a device key in a file: its id is derived
    // from the key.
    let key_dir = temp_dir("e2e");
    let key_path = key_dir.join("fake.key");
    let fake = device_key::load_or_create(&key_path)
        .unwrap()
        .public_key()
        .device_id();
    let mut fake_proc = tokio::process::Command::new(env!("CARGO_BIN_EXE_fake_device"))
        .args([
            "--url",
            &format!("ws://{addr}/lcp/v0/device"),
            "--key",
            key_path.to_str().unwrap(),
            "--name",
            "Fake One",
            // Ungated control, as an app before shading; protocol 2 has its
            // own test below.
            "--protocol",
            "1",
        ])
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn fake_device");

    // Controller A.
    let mut a = connect(addr, "/lcp/v0/control").await;
    let snap = wait_for(&mut a, 3, "snapshot", |v| v["type"] == "snapshot").await;
    let raw_snap = snap["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == "raw-1")
        .expect("raw device in snapshot")
        .clone();
    assert_eq!(raw_snap["online"], true);
    assert_eq!(raw_snap["state"]["gain"], 3);
    assert_eq!(
        raw_snap["info"]["future_field"], 1,
        "unknown fields are kept"
    );
    let p = wait_for(&mut raw, 3, "preview on", |v| v["type"] == "preview").await;
    assert_eq!(
        p["enabled"], true,
        "controller connected: preview must be on"
    );
    assert_eq!(p["fps"], tallycam_hub::hub::DEFAULT_PREVIEW_FPS);

    // The fake device appears (in the snapshot or as device_hello).
    let in_snap = snap["devices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["id"] == fake.as_str());
    if !in_snap {
        let hello = wait_for(&mut a, 5, "fake hello", |v| is(v, "device_hello", &fake)).await;
        let iso = hello["controls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "iso")
            .unwrap()
            .clone();
        assert_eq!(iso["scale"], "log");
        assert_eq!(iso["min"], 32.0);
        assert_eq!(iso["max"], 2176.0);
        assert_eq!(iso["enabled_when"]["exposure_mode"], "manual");
    }
    let fs = wait_for(&mut a, 5, "fake status", |v| is(v, "device_status", &fake)).await;
    // The fake device runs the core's clock discipline on a simulated network.
    assert_eq!(fs["clock"]["timescale"], "tai", "{fs}");
    assert_eq!(
        fs["clock"]["sources"].as_array().map(Vec::len),
        Some(8),
        "{fs}"
    );
    let pv = wait_for(&mut a, 5, "fake preview", |v| {
        is(v, "device_preview", &fake)
    })
    .await;
    assert!(pv["jpeg"].as_str().unwrap().len() > 100);
    assert_eq!(pv["width"], 480);

    // set -> state round trip.
    send(
        &mut a,
        json!({"type":"set","device":fake,"id":1,
                        "values":{"exposure_mode":"manual","iso":400,"shutter":100}}),
    )
    .await;
    let r = wait_for(&mut a, 3, "result 1", |v| {
        is(v, "device_result", &fake) && v["id"] == 1
    })
    .await;
    assert_eq!(r["ok"], true, "{r}");
    // The state echo was sent before the result; check the hub stored it.
    let mut b = connect(addr, "/lcp/v0/control").await;
    let snap_b = wait_for(&mut b, 3, "snapshot b", |v| v["type"] == "snapshot").await;
    let fake_b = snap_b["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["id"] == fake.as_str())
        .unwrap()
        .clone();
    assert_eq!(fake_b["state"]["exposure_mode"], "manual");
    assert_eq!(fake_b["state"]["iso"], 400);
    assert_eq!(fake_b["state"]["shutter"], 100);
    drop(b);

    // Clamp.
    send(
        &mut a,
        json!({"type":"set","device":fake,"id":2,"values":{"iso":12800}}),
    )
    .await;
    let st = wait_for(&mut a, 3, "clamped state", |v| {
        is(v, "device_state", &fake) && v["values"].get("iso").is_some()
    })
    .await;
    assert_eq!(st["values"]["iso"], 2176);
    let r = wait_for(&mut a, 3, "result 2", |v| {
        is(v, "device_result", &fake) && v["id"] == 2
    })
    .await;
    assert_eq!(r["ok"], false);
    assert!(r["error"].as_str().unwrap().contains("clamped"), "{r}");

    // enabled_when not met.
    send(
        &mut a,
        json!({"type":"set","device":fake,"id":3,"values":{"wb_temperature":3200}}),
    )
    .await;
    let r = wait_for(&mut a, 3, "result 3", |v| {
        is(v, "device_result", &fake) && v["id"] == 3
    })
    .await;
    assert_eq!(r["ok"], false);
    assert!(
        r["error"].as_str().unwrap().contains("only writable"),
        "{r}"
    );

    // Command.
    send(
        &mut a,
        json!({"type":"command","device":fake,"id":4,"name":"focus_at","args":{"x":0.25,"y":0.75}}),
    )
    .await;
    let r = wait_for(&mut a, 3, "result 4", |v| {
        is(v, "device_result", &fake) && v["id"] == 4
    })
    .await;
    assert_eq!(r["ok"], true, "{r}");

    // Message -> device log.
    send(
        &mut a,
        json!({"type":"message","device":fake,"text":"Pan left"}),
    )
    .await;
    let l = wait_for(&mut a, 3, "device log", |v| is(v, "device_log", &fake)).await;
    assert!(l["message"].as_str().unwrap().contains("Pan left"));

    // Tally: forwarded to the phone, broadcast, stored.
    send(
        &mut a,
        json!({"type":"tally","device":"raw-1","state":"program"}),
    )
    .await;
    let t = wait_for(&mut raw, 3, "tally at device", |v| v["type"] == "tally").await;
    assert_eq!(t["state"], "program");
    assert!(t.get("device").is_none(), "hub strips 'device'");
    wait_for(&mut a, 3, "device_tally", |v| {
        is(v, "device_tally", "raw-1") && v["state"] == "program"
    })
    .await;

    // Unknown device -> error to the sender.
    send(
        &mut a,
        json!({"type":"set","device":"nope","values":{"x":1}}),
    )
    .await;
    let e = wait_for(&mut a, 3, "error", |v| v["type"] == "error").await;
    assert_eq!(e["device"], "nope");

    // Ping.
    send(&mut raw, json!({"type":"ping","id":9})).await;
    let p = wait_for(&mut raw, 3, "pong", |v| v["type"] == "pong").await;
    assert_eq!(p["id"], 9);

    // Time sync is not the hub's business any more: a `time_request` is
    // ignored (no reply), and `status.clock` from the phone's own discipline
    // reaches the controllers unchanged.
    send(&mut raw, json!({"type":"time_request","id":41,"t1":1})).await;
    send(&mut raw, json!({"type":"ping","id":42})).await;
    let p = wait_for(&mut raw, 3, "pong after time_request", |v| {
        v["type"] == "pong" || v["type"] == "time_response"
    })
    .await;
    assert_eq!(p["type"], "pong", "no time_response: {p}");
    send(&mut raw, json!({"type":"status","thermal":"nominal",
                          "clock":{"synced":true,"health":"healthy","timescale":"tai","tai_utc_s":37,
                                   "stratum":3,"est_error_us":320,"max_error_us":3798,"selected":1,"combined":2,
                                   "sources":[{"state":"*","name":"ntp.ubuntu.com","addr":"192.0.2.1","stratum":2,
                                               "poll":6,"reach":255,"offset_us":-12,"error_us":3500,"future":true}],
                                   "future_field":"kept"}})).await;
    let s = wait_for(&mut a, 3, "status with clock", |v| {
        is(v, "device_status", "raw-1")
    })
    .await;
    assert_eq!(s["clock"]["synced"], true);
    assert_eq!(s["clock"]["health"], "healthy");
    assert_eq!(s["clock"]["tai_utc_s"], 37);
    assert_eq!(s["clock"]["max_error_us"], 3798);
    assert_eq!(s["clock"]["sources"][0]["state"], "*");
    assert_eq!(
        s["clock"]["sources"][0]["future"], true,
        "unknown source fields pass through"
    );
    assert_eq!(
        s["clock"]["future_field"], "kept",
        "unknown clock fields pass through"
    );
    assert!(s.get("time").is_none());

    // Last controller leaves -> preview off.
    a.close(None).await.unwrap();
    drop(a);
    let p = wait_for(&mut raw, 3, "preview off", |v| v["type"] == "preview").await;
    assert_eq!(
        p["enabled"], false,
        "no controller: preview must be off again"
    );

    // Reconnect with the same id replaces the old socket, keeps state and tally.
    let mut c = connect(addr, "/lcp/v0/control").await;
    wait_for(&mut c, 3, "snapshot c", |v| v["type"] == "snapshot").await;
    let mut raw2 = connect(addr, "/lcp/v0/device").await;
    send(
        &mut raw2,
        json!({"type":"hello","protocol":0,"device":{"id":"raw-1","name":"Raw again"},
                           "controls":[{"id":"gain","kind":"range","min":0,"max":10}]}),
    )
    .await;
    let t = wait_for(&mut raw2, 3, "tally on reconnect", |v| v["type"] == "tally").await;
    assert_eq!(t["state"], "program");
    // The old socket gets closed by the hub.
    let closed = timeout(Duration::from_secs(3), async {
        loop {
            match raw.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                _ => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "old device socket should be closed");
    let h = wait_for(&mut c, 3, "re-hello", |v| is(v, "device_hello", "raw-1")).await;
    assert_eq!(h["info"]["name"], "Raw again");
    // Disconnect -> offline but still listed with last-known state.
    raw2.close(None).await.unwrap();
    drop(raw2);
    wait_for(&mut c, 3, "offline", |v| is(v, "device_offline", "raw-1")).await;
    let mut d = connect(addr, "/lcp/v0/control").await;
    let snap = wait_for(&mut d, 3, "snapshot d", |v| v["type"] == "snapshot").await;
    let raw_snap = snap["devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == "raw-1")
        .unwrap()
        .clone();
    assert_eq!(raw_snap["online"], false);
    assert_eq!(raw_snap["state"]["gain"], 3);
    assert_eq!(raw_snap["tally"], "program");

    // Commands to an offline device are refused.
    send(
        &mut d,
        json!({"type":"set","device":"raw-1","values":{"gain":1}}),
    )
    .await;
    let e = wait_for(&mut d, 3, "offline error", |v| v["type"] == "error").await;
    assert!(e["message"].as_str().unwrap().contains("offline"));

    fake_proc.kill().await.ok();
    let _ = std::fs::remove_dir_all(&key_dir);
}

/// Shading with the real fake device at protocol 2: exposure set by any
/// controller (last write wins), image stats, and the production settings,
/// which a live camera applies when its stream stops.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shading_end_to_end() {
    let addr = start_hub().await;
    let key_dir = temp_dir("e2e-shading");
    let key_path = key_dir.join("fake.key");
    let fake = device_key::load_or_create(&key_path)
        .unwrap()
        .public_key()
        .device_id();
    let mut fake_proc = tokio::process::Command::new(env!("CARGO_BIN_EXE_fake_device"))
        .args([
            "--url",
            &format!("ws://{addr}/lcp/v0/device"),
            "--key",
            key_path.to_str().unwrap(),
        ])
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("spawn fake_device");

    let mut a = connect(addr, "/lcp/v0/control").await;
    let snap = wait_for(&mut a, 3, "snapshot", |v| v["type"] == "snapshot").await;
    assert_eq!(snap["production"]["mains_hz"], 50);
    wait_for(&mut a, 5, "fake device state", |v| {
        is(v, "device_state", &fake)
    })
    .await;

    // Any controller sets exposure; an older controller's holder is ignored.
    send(
        &mut a,
        json!({"type":"set","device":fake,"id":1,
               "values":{"exposure_mode":"manual","iso":400,"zoom":2.0}}),
    )
    .await;
    let r = wait_for(&mut a, 3, "set result", |v| {
        is(v, "device_result", &fake) && v["id"] == 1
    })
    .await;
    assert_eq!(r["ok"], true, "{r}");

    // Image statistics while a controller is connected, reacting to ISO.
    let st = wait_for(&mut a, 5, "image stats", |v| {
        is(v, "device_image_stats", &fake) && v["camera"]["iso"] == 400
    })
    .await;
    let dark = st["luma"]["mean"].as_f64().unwrap();
    assert!(st["luma"]["histogram"].as_str().unwrap().len() > 10);
    send(
        &mut a,
        json!({"type":"set","device":fake,"holder":{"id":"a1","label":"Shader A"},
               "values":{"iso":1600}}),
    )
    .await;
    let st = wait_for(&mut a, 5, "brighter stats", |v| {
        is(v, "device_image_stats", &fake) && v["camera"]["iso"] == 1600
    })
    .await;
    assert!(st["luma"]["mean"].as_f64().unwrap() > dark, "{st}");

    // Production: applied while idle.
    send(
        &mut a,
        json!({"type":"production","mains_hz":60,"shutter":"mains"}),
    )
    .await;
    let p = wait_for(&mut a, 3, "production", |v| v["type"] == "production").await;
    assert_eq!(p["mains_hz"], 60);
    wait_for(&mut a, 5, "shutter 1/60", |v| {
        is(v, "device_state", &fake) && v["values"]["shutter"] == 60
    })
    .await;

    // While live: accepted, applied when the stream stops.
    send(
        &mut a,
        json!({"type":"set","device":fake,"values":{"live":true}}),
    )
    .await;
    wait_for(&mut a, 5, "live", |v| {
        is(v, "device_status", &fake)
            && matches!(v["stream"]["state"].as_str(), Some("live" | "connecting"))
    })
    .await;
    send(
        &mut a,
        json!({"type":"production","mains_hz":50,"shutter":"mains"}),
    )
    .await;
    let p = wait_for(&mut a, 3, "production while live", |v| {
        v["type"] == "production" || v["type"] == "error"
    })
    .await;
    assert_eq!(p["type"], "production", "{p}");
    assert_eq!(p["mains_hz"], 50);
    send(
        &mut a,
        json!({"type":"set","device":fake,"values":{"live":false}}),
    )
    .await;
    wait_for(&mut a, 5, "shutter 1/50 after the stream", |v| {
        is(v, "device_state", &fake) && v["values"]["shutter"] == 50
    })
    .await;

    fake_proc.kill().await.ok();
    let _ = std::fs::remove_dir_all(&key_dir);
}

/// Plain HTTP/1.1 GET; returns (status, content type, body).
async fn http_get(addr: SocketAddr, path: &str) -> (u16, String, String) {
    http_request(addr, "GET", path).await
}

async fn http_request(addr: SocketAddr, method: &str, path: &str) -> (u16, String, String) {
    http_request_with(addr, method, path, "").await
}

/// `extra`: more header lines, each ending in "\r\n".
async fn http_request_with(
    addr: SocketAddr,
    method: &str,
    path: &str,
    extra: &str,
) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).await.unwrap();
    s.write_all(
        format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut raw = Vec::new();
    timeout(Duration::from_secs(5), s.read_to_end(&mut raw))
        .await
        .unwrap()
        .unwrap();
    let raw = String::from_utf8(raw).unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let ctype = head
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-type:")
                .map(|v| v.trim().to_string())
        })
        .unwrap_or_default();
    assert!(
        !head
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked"),
        "{head}"
    );
    (status, ctype, body.to_string())
}

/// Decodes the hub's QR SVG (one unit per module, dark modules as
/// `M{x} {y}h{w}v{h}H{x}V{y}` path segments).
fn decode_qr_svg(svg: &str) -> String {
    let dim = |attr: &str| -> usize {
        let at = svg.find(&format!(" {attr}=\"")).unwrap() + attr.len() + 3;
        svg[at..].split('"').next().unwrap().parse().unwrap()
    };
    let (w, h) = (dim("width"), dim("height"));
    let d_at = svg.find(" d=\"").unwrap() + 4;
    let d = svg[d_at..].split('"').next().unwrap();
    let mut dark = vec![false; w * h];
    for seg in d.split('M').filter(|s| !s.is_empty()) {
        let nums: Vec<usize> = seg
            .split(|c: char| !c.is_ascii_digit())
            .filter(|n| !n.is_empty())
            .map(|n| n.parse().unwrap())
            .collect();
        let (x, y, rw, rh) = (nums[0], nums[1], nums[2], nums[3]);
        for yy in y..y + rh {
            for xx in x..x + rw {
                dark[yy * w + xx] = true;
            }
        }
    }
    const SCALE: usize = 6;
    let mut img = rqrr::PreparedImage::prepare_from_bitmap(w * SCALE, h * SCALE, |x, y| {
        dark[(y / SCALE) * w + x / SCALE]
    });
    let grids = img.detect_grids();
    assert_eq!(grids.len(), 1, "one QR code in the SVG");
    grids[0].decode().unwrap().1
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provisioning_endpoints() {
    let addr = start_hub_with(|addr| {
        let mut p = Provisioning::new(addr);
        p.public_url = Some("ws://192.0.2.10:8090/lcp/v0/device".into());
        p
    })
    .await;

    let (status, ctype, body) = http_get(addr, "/api/info").await;
    assert_eq!(status, 200);
    assert!(ctype.starts_with("application/json"), "{ctype}");
    let info: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(info["port"], addr.port());
    assert_eq!(info["scheme"], "tallycam");
    let t = &info["targets"][0];
    assert_eq!(t["label"], "192.0.2.10");
    assert_eq!(t["control_url"], "ws://192.0.2.10:8090/lcp/v0/device");
    assert_eq!(
        t["srt_url"],
        "srt://192.0.2.10:9000?streamid={device}&latency=100"
    );
    let link = t["deep_link"].as_str().unwrap().to_string();
    assert!(
        link.starts_with("tallycam://connect?control=ws%3A%2F%2F192.0.2.10"),
        "{link}"
    );

    let (status, ctype, svg) = http_get(addr, "/api/connect.svg").await;
    assert_eq!(status, 200);
    assert_eq!(ctype, "image/svg+xml");
    assert!(svg.contains("<svg") && svg.ends_with("</svg>"), "{svg}");
    assert_eq!(
        decode_qr_svg(&svg),
        link,
        "the QR code carries the deep link"
    );

    let (status, _, _) = http_get(addr, "/api/connect.svg?target=5").await;
    assert_eq!(status, 404);

    // A hub on loopback advertises nothing: localhost is useless to a phone.
    let local = start_hub().await;
    let (_, _, body) = http_get(local, "/api/info").await;
    let info: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(info["targets"].as_array().map(Vec::len), Some(0), "{info}");
    assert_eq!(http_get(local, "/api/connect.svg").await.0, 404);

    // On 0.0.0.0 the LAN addresses never include loopback or link-local.
    let any = Provisioning::new(SocketAddr::from(([0, 0, 0, 0], 8090))).info();
    for a in &any.addresses {
        assert!(!a.starts_with("127.") && !a.starts_with("169.254."), "{a}");
    }
    for t in &any.targets {
        assert!(
            t.control_url.ends_with(":8090/lcp/v0/device"),
            "{}",
            t.control_url
        );
    }
}

async fn start_invite_hub() -> (SocketAddr, Arc<Hub>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hub = Arc::new(Hub::new());
    hub.set_invite_required(true);
    // Loopback is never advertised; name it as the public URL so invites
    // point here.
    let mut provisioning = Provisioning::new(addr);
    provisioning.public_url = Some(format!("ws://{addr}/lcp/v0/device"));
    let app = tallycam_hub::server::router(hub.clone(), provisioning);
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (addr, hub)
}

fn keyed_hello(key: &SoftwareKey, name: &str) -> Value {
    let public = key.public_key();
    json!({"type":"hello","protocol":1,
           "device":{"id":public.device_id(),"name":name},
           "public_key":public.to_base64()})
}

/// Says hello with `key`, answers the challenge, and returns the first
/// message after it: `tally` when admitted, `message` when refused.
async fn handshake(ws: &mut Ws, key: &SoftwareKey, name: &str) -> Value {
    send(ws, keyed_hello(key, name)).await;
    let c = wait_for(ws, 3, "challenge", |v| v["type"] == "challenge").await;
    let signature = key.sign_auth(c["server"].as_str().unwrap(), c["nonce"].as_str().unwrap());
    send(ws, json!({"type":"auth","signature":signature})).await;
    wait_for(ws, 3, "tally or refusal", |v| {
        v["type"] == "tally" || v["type"] == "message"
    })
    .await
}

async fn expect_closed(ws: &mut Ws) {
    let closed = timeout(Duration::from_secs(3), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => return,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "a refused socket is closed");
}

/// Device keys and invites over the real WebSocket route: an unknown phone
/// without a token is told why and disconnected; one with a token joins and
/// its key is bound, so it comes back later with the key alone. Another key
/// can not take its id, a replayed signature does not work, an app without
/// a key is told to update, and forgetting the device revokes the key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_keys_and_invites() {
    let (addr, hub) = start_invite_hub().await;
    let key = device_key::generate();
    let id = key.public_key().device_id();

    // No token: a message with the reason, then the socket closes.
    let mut ws = connect(addr, "/lcp/v0/device").await;
    let m = handshake(&mut ws, &key, "Invited").await;
    assert!(m["text"].as_str().unwrap().contains("not invited"), "{m}");
    expect_closed(&mut ws).await;
    assert!(hub.devices().is_empty());

    // An app without a key is told to update.
    let mut ws = connect(addr, "/lcp/v0/device").await;
    send(
        &mut ws,
        json!({"type":"hello","protocol":0,"device":{"id":"legacy-1"}}),
    )
    .await;
    let m = wait_for(&mut ws, 3, "refusal", |v| v["type"] == "message").await;
    assert!(
        m["text"].as_str().unwrap().contains("update the app"),
        "{m}"
    );
    expect_closed(&mut ws).await;

    // With a token: admitted, listed with its key fingerprint.
    let token = hub.create_invite(tallycam_hub::hub::DEFAULT_INVITE_TTL);
    let path = format!("/lcp/v0/device?token={token}");
    let mut ws = connect(addr, &path).await;
    let m = handshake(&mut ws, &key, "Invited").await;
    assert_eq!(m["type"], "tally", "{m}");
    let listed = hub.devices().into_iter().find(|d| d.id == id).unwrap();
    assert!(listed.online);
    assert_eq!(listed.key_fingerprint, Some(key.public_key().fingerprint()));
    // The host the phone used is kept, for media URLs pushed later.
    assert_eq!(hub.device_host(&id), Some(addr.to_string()));
    drop(ws);

    // Without a token later: the key is enough.
    let mut ws = connect(addr, "/lcp/v0/device").await;
    assert_eq!(handshake(&mut ws, &key, "Invited").await["type"], "tally");
    drop(ws);

    // Another key claiming the id: refused (the id is not derived from it).
    let thief = device_key::generate();
    let mut ws = connect(addr, &path).await;
    let mut hello = keyed_hello(&thief, "Thief");
    hello["device"]["id"] = id.clone().into();
    send(&mut ws, hello).await;
    let m = wait_for(&mut ws, 3, "refusal", |v| v["type"] == "message").await;
    assert!(m["text"].as_str().unwrap().contains("not usable"), "{m}");
    expect_closed(&mut ws).await;

    // The right key, but a signature from an earlier challenge: refused.
    let mut ws = connect(addr, "/lcp/v0/device").await;
    send(&mut ws, keyed_hello(&key, "Invited")).await;
    let first = wait_for(&mut ws, 3, "challenge", |v| v["type"] == "challenge").await;
    let old = key.sign_auth(
        first["server"].as_str().unwrap(),
        first["nonce"].as_str().unwrap(),
    );
    drop(ws);
    let mut ws = connect(addr, "/lcp/v0/device").await;
    send(&mut ws, keyed_hello(&key, "Invited")).await;
    wait_for(&mut ws, 3, "challenge", |v| v["type"] == "challenge").await;
    send(&mut ws, json!({"type":"auth","signature":old})).await;
    let m = wait_for(&mut ws, 3, "refusal", |v| v["type"] == "message").await;
    assert!(
        m["text"].as_str().unwrap().contains("did not verify"),
        "{m}"
    );
    expect_closed(&mut ws).await;

    // The spent token does not admit another phone.
    let other = device_key::generate();
    let mut ws = connect(addr, &path).await;
    let m = handshake(&mut ws, &other, "Other").await;
    assert!(
        m["text"].as_str().unwrap().contains("used or expired"),
        "{m}"
    );

    // Forget it over HTTP: the open connection closes, the key alone no
    // longer admits it.
    let mut ws = connect(addr, "/lcp/v0/device").await;
    assert_eq!(handshake(&mut ws, &key, "Invited").await["type"], "tally");
    let (status, _, _) = http_request(addr, "DELETE", &format!("/api/devices/{id}")).await;
    assert_eq!(status, 204);
    expect_closed(&mut ws).await;
    assert!(hub.devices().is_empty());
    let (status, _, _) = http_request(addr, "DELETE", &format!("/api/devices/{id}")).await;
    assert_eq!(status, 404);
    let mut ws = connect(addr, "/lcp/v0/device").await;
    let m = handshake(&mut ws, &key, "Invited").await;
    assert!(m["text"].as_str().unwrap().contains("not invited"), "{m}");
}

/// The real `fake_device` with a key file against a hub that requires
/// invites: admitted with the invite in its URL, and again after a restart
/// with the same key file and no token. `POST /api/invite` hands out the
/// link.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fake_device_joins_with_an_invite_and_returns_with_its_key() {
    let (addr, hub) = start_invite_hub().await;
    let dir = temp_dir("fake-key");
    let key_path = dir.join("device.key");

    let (status, _, body) = http_request(addr, "POST", "/api/invite").await;
    assert_eq!(status, 200, "{body}");
    let invite: Value = serde_json::from_str(&body).unwrap();
    let control_url = invite["control_url"].as_str().unwrap().to_string();
    assert!(control_url.contains("?token="), "{control_url}");
    assert!(invite["deep_link"]
        .as_str()
        .unwrap()
        .starts_with("tallycam://connect?control="));
    assert!(invite["qr_svg"].as_str().unwrap().contains("<svg"));

    let spawn = |url: String| {
        tokio::process::Command::new(env!("CARGO_BIN_EXE_fake_device"))
            .args(["--url", &url, "--key", key_path.to_str().unwrap()])
            .kill_on_drop(true)
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn fake_device")
    };
    let wait_online = |online: bool| {
        let hub = hub.clone();
        async move {
            timeout(Duration::from_secs(5), async {
                while hub.devices().iter().any(|d| d.online) != online {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("device online should become {online}"));
        }
    };

    let mut first = spawn(control_url);
    wait_online(true).await;
    let devices = hub.devices();
    assert_eq!(devices.len(), 1);
    let id = devices[0].id.clone();
    assert_eq!(
        Some(id.clone()),
        tallycam_hub::device_key::device_id(
            &device_key::load_or_create(&key_path)
                .unwrap()
                .public_key()
                .to_base64()
        )
    );
    first.kill().await.ok();
    wait_online(false).await;

    // Same key file, no token: known by its key.
    let mut second = spawn(format!("ws://{addr}/lcp/v0/device"));
    wait_online(true).await;
    assert_eq!(hub.devices().len(), 1);
    assert_eq!(hub.devices()[0].id, id);
    second.kill().await.ok();
    let _ = std::fs::remove_dir_all(&dir);
}

/// With an operator token, invites, forgetting and the controller socket
/// need it; the phones' endpoint does not (they prove a key instead).
#[tokio::test]
async fn operator_token_guards_invites_forget_and_control() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hub = Arc::new(Hub::new());
    hub.set_invite_required(true);
    let options = tallycam_hub::server::ServerOptions {
        operator_token: Some("s3cret".into()),
    };
    // A loopback hub advertises no LAN address; name one for the invite.
    let mut provisioning = Provisioning::new(addr);
    provisioning.public_url = Some(format!("ws://{addr}/lcp/v0/device"));
    let app = tallycam_hub::server::router_with(hub, provisioning, options);
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    let (status, _, _) = http_request(addr, "POST", "/api/invite").await;
    assert_eq!(status, 401, "invite without the token");
    let (status, _, _) =
        http_request_with(addr, "POST", "/api/invite", "X-Operator-Token: wrong\r\n").await;
    assert_eq!(status, 401, "invite with a wrong token");
    let (status, _, body) =
        http_request_with(addr, "POST", "/api/invite", "X-Operator-Token: s3cret\r\n").await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("deep_link"));

    let (status, _, _) = http_request(addr, "DELETE", "/api/devices/nobody").await;
    assert_eq!(status, 401, "forget without the token");
    let (status, _, _) = http_request_with(
        addr,
        "DELETE",
        "/api/devices/nobody",
        "X-Operator-Token: s3cret\r\n",
    )
    .await;
    assert_eq!(status, 404, "unknown device, but allowed");

    assert!(
        connect_async(format!("ws://{addr}{CONTROL_PATH}"))
            .await
            .is_err(),
        "controller without the token"
    );
    let mut ctl = connect(addr, &format!("{CONTROL_PATH}?token=s3cret")).await;
    let snap = wait_for(&mut ctl, 5, "snapshot", |m| m["type"] == "snapshot").await;
    assert_eq!(snap["devices"], json!([]));
}
