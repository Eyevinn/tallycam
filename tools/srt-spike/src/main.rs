//! srt-spike: interop and latency tests of the core's Rust SRT sender
//! (tallycam-srt) against libsrt.
//!
//! Modes:
//! - `send URL`: generated TS through the Rust sender; prints events and
//!   stats every second.
//! - `gen`: the same generated TS on stdout in real time, for a libsrt
//!   sender (`srt-spike gen | srt-live-transmit file://con srt://...`).
//! - `measure`: reads the received TS on stdin (from `srt-live-transmit
//!   srt://:port?mode=listener file://con` or `srtsrc ! fdsink`) and prints
//!   per-second arrivals, lost and duplicate packets, and send-to-receive
//!   latency (both ends on this machine, CLOCK_REALTIME).
//! - `proxy LISTEN TARGET`: a UDP relay that drops and delays packets in
//!   both directions.
//!
//! Generated TS: one PID, frames at `--fps` with a keyframe (8x the size)
//! every `--gop-s`; every TS packet carries a sequence number, its frame and
//! the wall-clock time it was handed to the sender.

use std::io::{Read, Write};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tallycam_srt::{CallerConfig, SenderOptions, SrtEvent, SrtObserver, SrtSender};

const USAGE: &str = "\
Usage:
  srt-spike send URL [--bitrate-kbps K] [--fps N] [--gop-s S] [--duration-s S] [--reconnect]
  srt-spike gen [--bitrate-kbps K] [--fps N] [--gop-s S] [--duration-s S]
  srt-spike measure [--quiet]
  srt-spike proxy LISTEN_ADDR TARGET_ADDR [--loss PCT] [--delay-ms MS] [--seed N]
";

const MAGIC: &[u8; 4] = b"TCSP";

struct GenArgs {
    bitrate_kbps: u64,
    fps: u64,
    gop_s: u64,
    duration_s: Option<u64>,
}

fn wall_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

/// TS packets of one generated frame, stamped now.
struct Generator {
    args: GenArgs,
    seq: u64,
    frame: u32,
    cc: u8,
}

impl Generator {
    fn new(args: GenArgs) -> Self {
        Generator {
            args,
            seq: 0,
            frame: 0,
            cc: 0,
        }
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_nanos(1_000_000_000 / self.args.fps)
    }

    fn next_frame(&mut self, out: &mut Vec<u8>) {
        out.clear();
        let per_gop = self.args.fps * self.args.gop_s;
        // Average bytes per frame with one keyframe of 8 units per GOP.
        let unit = self.args.bitrate_kbps * 1000 / 8 * self.args.gop_s / (per_gop + 7);
        let bytes = if u64::from(self.frame) % per_gop == 0 {
            unit * 8
        } else {
            unit
        };
        let packets = bytes.div_ceil(188).max(1);
        let now = wall_ns();
        for _ in 0..packets {
            let mut p = [0xffu8; 188];
            p[0] = 0x47;
            p[1] = 0x01;
            p[2] = 0x00;
            p[3] = 0x10 | (self.cc & 0x0f);
            self.cc = self.cc.wrapping_add(1);
            p[4..8].copy_from_slice(MAGIC);
            p[8..16].copy_from_slice(&self.seq.to_be_bytes());
            p[16..24].copy_from_slice(&now.to_be_bytes());
            p[24..28].copy_from_slice(&self.frame.to_be_bytes());
            self.seq += 1;
            out.extend_from_slice(&p);
        }
        self.frame = self.frame.wrapping_add(1);
    }
}

fn parse_gen(args: &mut Vec<String>) -> GenArgs {
    GenArgs {
        bitrate_kbps: take_opt(args, "--bitrate-kbps").map_or(5000, |v| v.parse().unwrap()),
        fps: take_opt(args, "--fps").map_or(30, |v| v.parse().unwrap()),
        gop_s: take_opt(args, "--gop-s").map_or(2, |v| v.parse().unwrap()),
        duration_s: take_opt(args, "--duration-s").map(|v| v.parse().unwrap()),
    }
}

fn take_opt(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.remove(i);
    Some(args.remove(i))
}

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    match args.iter().position(|a| a == name) {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// send
// ---------------------------------------------------------------------------

struct Printer {
    start: Instant,
    connected: AtomicBool,
    /// Sum of queue-to-wire times of frame starts, ns.
    queue_ns: AtomicU64,
    queue_max_ns: AtomicU64,
    frames: AtomicU64,
}

impl SrtObserver for Printer {
    fn on_event(&self, event: SrtEvent) {
        match event {
            SrtEvent::Connected { .. } => self.connected.store(true, Ordering::SeqCst),
            SrtEvent::Disconnected { .. } => self.connected.store(false, Ordering::SeqCst),
            _ => {}
        }
        println!(
            "[{:8.3}] event {event:?}",
            self.start.elapsed().as_secs_f64()
        );
    }

    fn on_frame_sent(&self, _capture_ns: i64, enqueued_ns: i64, sent_ns: i64, last: bool) {
        if !last {
            let d = (sent_ns - enqueued_ns).max(0) as u64;
            self.queue_ns.fetch_add(d, Ordering::Relaxed);
            self.queue_max_ns.fetch_max(d, Ordering::Relaxed);
            self.frames.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn send(mut args: Vec<String>) {
    let gen = parse_gen(&mut args);
    let reconnect = take_flag(&mut args, "--reconnect");
    let url = args.first().expect("URL").clone();
    let mut cfg = CallerConfig::from_url(&url).unwrap_or_else(|e| panic!("{e}"));
    cfg.reconnect = reconnect;
    let duration = gen.duration_s.map(Duration::from_secs);
    let printer = Arc::new(Printer {
        start: Instant::now(),
        connected: AtomicBool::new(false),
        queue_ns: AtomicU64::new(0),
        queue_max_ns: AtomicU64::new(0),
        frames: AtomicU64::new(0),
    });
    let sender = SrtSender::start(cfg, SenderOptions::default(), printer.clone()).unwrap();
    let mut g = Generator::new(gen);
    let interval = g.frame_interval();
    let mut buf = Vec::new();
    let start = Instant::now();
    let mut next = start;
    let mut next_print = start + Duration::from_secs(1);
    let (mut sent_calls, mut refused) = (0u64, 0u64);
    let mut send_call_max = Duration::ZERO;
    loop {
        let now = Instant::now();
        if duration.is_some_and(|d| now - start >= d) {
            break;
        }
        if now >= next {
            next += interval;
            g.next_frame(&mut buf);
            let t = Instant::now();
            let r = sender.send(&buf, 1, 0);
            send_call_max = send_call_max.max(t.elapsed());
            match r {
                Ok(_) => sent_calls += 1,
                Err(_) => refused += 1,
            }
        }
        if now >= next_print {
            next_print += Duration::from_secs(1);
            let s = sender.stats();
            let frames = printer.frames.swap(0, Ordering::Relaxed).max(1);
            println!(
                "[{:8.3}] stats connected={} rtt={:.1}ms sent={} retrans={} naks={} lost={} dropped={} inflight={} lat={}ms bytes={} conns={} calls={} refused={} send_call_max={}us queue_avg={}us queue_max={}us",
                printer.start.elapsed().as_secs_f64(),
                s.connected, s.rtt_ms, s.packets_sent, s.retransmitted_packets, s.received_naks,
                s.lost_packets, s.dropped_packets, s.packets_in_flight, s.negotiated_latency_ms,
                s.bytes_sent, s.connections, sent_calls, refused, send_call_max.as_micros(),
                printer.queue_ns.swap(0, Ordering::Relaxed) / frames / 1000,
                printer.queue_max_ns.swap(0, Ordering::Relaxed) / 1000,
            );
            send_call_max = Duration::ZERO;
        }
        std::thread::sleep(
            next.saturating_duration_since(Instant::now())
                .min(Duration::from_millis(5)),
        );
    }
    let t = Instant::now();
    sender.close();
    println!(
        "closed in {} us; final {:?}",
        t.elapsed().as_micros(),
        sender.stats()
    );
}

// ---------------------------------------------------------------------------
// gen
// ---------------------------------------------------------------------------

fn gen(mut args: Vec<String>) {
    let gen = parse_gen(&mut args);
    let duration = gen.duration_s.map(Duration::from_secs);
    let mut g = Generator::new(gen);
    let interval = g.frame_interval();
    let mut buf = Vec::new();
    let mut out = std::io::stdout().lock();
    let start = Instant::now();
    let mut next = start;
    loop {
        if duration.is_some_and(|d| start.elapsed() >= d) {
            break;
        }
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
        next += interval;
        g.next_frame(&mut buf);
        if out.write_all(&buf).and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// measure
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Window {
    packets: u64,
    lost: u64,
    dups: u64,
    lat_us: Vec<u64>,
}

fn pct(v: &mut [u64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_unstable();
    let i = ((v.len() - 1) as f64 * p).round() as usize;
    v[i] as f64 / 1000.0
}

fn measure(mut args: Vec<String>) {
    let quiet = take_flag(&mut args, "--quiet");
    let mut stdin = std::io::stdin().lock();
    let mut buf = vec![0u8; 188 * 64];
    let mut have = 0usize;
    let mut expect: Option<u64> = None;
    let mut win = Window::default();
    let mut total = Window::default();
    let mut all_lat: Vec<u64> = Vec::new();
    let start = Instant::now();
    let mut next_print = Duration::from_secs(1);
    let mut last_arrival: Option<Instant> = None;
    let mut max_gap = Duration::ZERO;
    loop {
        let n = match stdin.read(&mut buf[have..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let now_ns = wall_ns();
        let arrival = Instant::now();
        if let Some(last) = last_arrival {
            max_gap = max_gap.max(arrival - last);
        }
        last_arrival = Some(arrival);
        have += n;
        let mut off = 0;
        // Resync on the sync byte.
        while off + 188 <= have {
            if buf[off] != 0x47 {
                off += 1;
                continue;
            }
            let p = &buf[off..off + 188];
            off += 188;
            if &p[4..8] != MAGIC {
                continue;
            }
            let seq = u64::from_be_bytes(p[8..16].try_into().unwrap());
            let sent = u64::from_be_bytes(p[16..24].try_into().unwrap());
            win.packets += 1;
            match expect {
                Some(e) if seq > e => win.lost += seq - e,
                Some(e) if seq < e => win.dups += 1,
                _ => {}
            }
            if expect.is_none_or(|e| seq >= e) {
                expect = Some(seq + 1);
            }
            win.lat_us.push(now_ns.saturating_sub(sent) / 1000);
        }
        buf.copy_within(off..have, 0);
        have -= off;
        if start.elapsed() >= next_print {
            next_print += Duration::from_secs(1);
            let mut l = std::mem::take(&mut win.lat_us);
            if !quiet {
                println!(
                    "[{:6.1}] packets={} lost={} dups={} latency p50={:.1} p95={:.1} max={:.1} ms",
                    start.elapsed().as_secs_f64(),
                    win.packets,
                    win.lost,
                    win.dups,
                    pct(&mut l, 0.5),
                    pct(&mut l, 0.95),
                    pct(&mut l, 1.0)
                );
            }
            total.packets += win.packets;
            total.lost += win.lost;
            total.dups += win.dups;
            all_lat.extend(l);
            win = Window::default();
        }
    }
    total.packets += win.packets;
    total.lost += win.lost;
    total.dups += win.dups;
    all_lat.extend(win.lat_us);
    println!(
        "SUMMARY packets={} lost={} dups={} latency p50={:.1} p95={:.1} p99={:.1} max={:.1} ms, longest arrival gap {} ms",
        total.packets,
        total.lost,
        total.dups,
        pct(&mut all_lat, 0.5),
        pct(&mut all_lat, 0.95),
        pct(&mut all_lat, 0.99),
        pct(&mut all_lat, 1.0),
        max_gap.as_millis()
    );
}

// ---------------------------------------------------------------------------
// proxy
// ---------------------------------------------------------------------------

/// xorshift64*: deterministic loss without a dependency.
struct Rng(u64);

impl Rng {
    fn chance(&mut self, pct: f64) -> bool {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let v = self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11;
        (v as f64 / (1u64 << 53) as f64) * 100.0 < pct
    }
}

fn proxy(mut args: Vec<String>) {
    let loss: f64 = take_opt(&mut args, "--loss").map_or(0.0, |v| v.parse().unwrap());
    let delay =
        Duration::from_millis(take_opt(&mut args, "--delay-ms").map_or(0, |v| v.parse().unwrap()));
    let seed: u64 =
        take_opt(&mut args, "--seed").map_or(0x9e37_79b9_7f4a_7c15, |v| v.parse().unwrap());
    let listen: SocketAddr = args[0].parse().expect("LISTEN_ADDR");
    let target: SocketAddr = args[1].parse().expect("TARGET_ADDR");
    let front = Arc::new(UdpSocket::bind(listen).unwrap());
    let back = Arc::new(UdpSocket::bind("127.0.0.1:0").unwrap());
    back.connect(target).unwrap();
    let client: Arc<std::sync::Mutex<Option<SocketAddr>>> = Arc::default();
    println!(
        "proxy {listen} -> {target}, loss {loss}% each way, delay {} ms each way",
        delay.as_millis()
    );
    let counters = Arc::new([
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
        AtomicU64::new(0),
    ]);

    // Client -> target.
    let (tx_up, rx_up) = mpsc::channel::<(Instant, Vec<u8>)>();
    {
        let (front, client, counters) = (front.clone(), client.clone(), counters.clone());
        std::thread::spawn(move || {
            let mut rng = Rng(seed);
            let mut buf = [0u8; 2048];
            loop {
                let Ok((n, from)) = front.recv_from(&mut buf) else {
                    continue;
                };
                *client.lock().unwrap() = Some(from);
                counters[0].fetch_add(1, Ordering::Relaxed);
                if n >= 16 && buf[0] & 0x80 != 0 {
                    // Handshake (0) and shutdown (5) from the caller.
                    let kind = u16::from_be_bytes([buf[0], buf[1]]) & 0x7fff;
                    if kind == 0 || kind == 5 {
                        let name = if kind == 0 { "handshake" } else { "SHUTDOWN" };
                        println!(
                            "[{:.3}] up {name} from {from}",
                            wall_ns() as f64 / 1e9 % 1000.0
                        );
                    }
                }
                if rng.chance(loss) {
                    counters[1].fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let _ = tx_up.send((Instant::now() + delay, buf[..n].to_vec()));
            }
        });
    }
    {
        let back = back.clone();
        std::thread::spawn(move || {
            for (due, data) in rx_up {
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
                let _ = back.send(&data);
            }
        });
    }
    // Target -> client.
    let (tx_down, rx_down) = mpsc::channel::<(Instant, Vec<u8>)>();
    {
        let (back, counters) = (back.clone(), counters.clone());
        std::thread::spawn(move || {
            let mut rng = Rng(seed ^ 0xdead_beef);
            let mut buf = [0u8; 2048];
            loop {
                // Errors: port unreachable while the target restarts.
                let Ok(n) = back.recv(&mut buf) else {
                    continue;
                };
                counters[2].fetch_add(1, Ordering::Relaxed);
                if rng.chance(loss) {
                    counters[3].fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let _ = tx_down.send((Instant::now() + delay, buf[..n].to_vec()));
            }
        });
    }
    {
        let front = front.clone();
        std::thread::spawn(move || {
            for (due, data) in rx_down {
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
                if let Some(c) = *client.lock().unwrap() {
                    let _ = front.send_to(&data, c);
                }
            }
        });
    }
    loop {
        std::thread::sleep(Duration::from_secs(5));
        println!(
            "proxy up {} (dropped {}), down {} (dropped {})",
            counters[0].load(Ordering::Relaxed),
            counters[1].load(Ordering::Relaxed),
            counters[2].load(Ordering::Relaxed),
            counters[3].load(Ordering::Relaxed)
        );
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprint!("{USAGE}");
        std::process::exit(2);
    }
    let mode = args.remove(0);
    match mode.as_str() {
        "send" => send(args),
        "gen" => gen(args),
        "measure" => measure(args),
        "proxy" => proxy(args),
        _ => {
            eprint!("{USAGE}");
            std::process::exit(2);
        }
    }
}
