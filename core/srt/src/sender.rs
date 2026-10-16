//! The SRT caller: one thread owns the UDP socket and the srt-protocol
//! engine (sans-IO), and waits in `poll(2)` on the socket and a wake pipe.
//! Payloads come in through a lock-free queue, so `send` never blocks on
//! the network or the engine.

use std::collections::{HashSet, VecDeque};
use std::io::{self, ErrorKind};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::os::fd::AsRawFd;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use log::{debug, info, warn};
use srt_protocol::connection::{Action, DuplexConnection, Input};
use srt_protocol::options::{
    DataRate, KeySize, LiveBandwidthMode, PacketCount, PacketSize, Passphrase,
};
use srt_protocol::packet::{
    Acknowledgement, ControlPacket, ControlTypes, HandshakeControlInfo, Packet, SeqNumber,
    ShakeType, SrtControlPacket, TimeStamp,
};
use srt_protocol::protocol::pending_connection::connect::Connect;
use srt_protocol::protocol::pending_connection::ConnectionResult;
use srt_protocol::settings::{ConnInitSettings, KeyMaterialRefreshSettings, KeySettings};

use crate::config::CallerConfig;
use crate::queue::{PushError, SpscQueue, MARK_FRAME_END, MARK_FRAME_START};

/// IPv4 + UDP header bytes, counted in `bytes_sent` like the Swift sender.
const IP_UDP_HEADER: u64 = 28;
/// Handshake packets are repeated this often until answered (libsrt: 250 ms).
const HANDSHAKE_REPEAT: Duration = Duration::from_millis(250);
/// Wait between connection attempts when reconnecting.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
/// With ICMP port unreachable and nothing heard for this long, the listener
/// is gone; do not wait for the full peer idle timeout.
const UNREACHABLE_GRACE: Duration = Duration::from_millis(1500);
/// Payload slots between the producer and the SRT thread (1.3 MB).
const QUEUE_SLOTS: usize = 1024;
/// SRT send buffer, packets (libsrt's default SRTO_SNDBUF is 8192 packets).
const SEND_BUFFER_PACKETS: u32 = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SrtEvent {
    /// A connection attempt started (1 = the first).
    Connecting { attempt: u32 },
    /// Handshake done; `latency_ms` is what both sides agreed to (TSBPD).
    Connected { latency_ms: u32, peer: String },
    /// Not connected any more. `reconnecting`: another attempt follows.
    Disconnected { reason: String, reconnecting: bool },
}

/// Called on the SRT thread; keep the work short (hop to your own queue).
pub trait SrtObserver: Send + Sync {
    fn on_event(&self, event: SrtEvent);
    /// The first (`last` false) or last payload of a video frame went out
    /// for the first time. Host ns, from [`SenderOptions::host_now_ns`].
    fn on_frame_sent(&self, capture_ns: i64, enqueued_ns: i64, sent_ns: i64, last: bool);
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SrtStats {
    pub connected: bool,
    /// Smoothed RTT the listener reports in its ACKs, ms.
    pub rtt_ms: f64,
    /// Data packets sent again (NAK or timeout).
    pub retransmitted_packets: u64,
    /// NAK reports received.
    pub received_naks: u64,
    /// Packets the listener reported lost.
    pub lost_packets: u64,
    /// Packets dropped by the sender: too late to play out, or the queue
    /// to the SRT thread was full.
    pub dropped_packets: u64,
    /// Sent but not acknowledged yet.
    pub packets_in_flight: u64,
    /// TSBPD latency both sides agreed to, ms; 0 before the first connect.
    pub negotiated_latency_ms: u32,
    /// Bytes handed to the network, with IP/UDP headers; all connections.
    pub bytes_sent: u64,
    /// Unique data packets sent; all connections.
    pub packets_sent: u64,
    /// Successful handshakes so far.
    pub connections: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SendError {
    NotConnected,
    QueueFull,
    /// Two threads called `send` at once; calls must be serialized.
    Busy,
}

pub struct SenderOptions {
    /// Host clock for the frame-sent reports and the queue timestamps; it
    /// must be the clock `send` callers pass `now_ns` from. On Apple
    /// platforms Instant is mach_absolute_time based like the capture clock.
    pub host_now_ns: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl Default for SenderOptions {
    fn default() -> Self {
        let epoch = Instant::now();
        SenderOptions {
            host_now_ns: Arc::new(move || epoch.elapsed().as_nanos() as i64),
        }
    }
}

/// State shared by the handle and the SRT thread. All atomics: `send` and
/// `stats` never lock.
struct Shared {
    queue: SpscQueue,
    closed: AtomicBool,
    connected: AtomicBool,
    parked: AtomicBool,
    waker: Waker,
    // Stats.
    rtt_us: AtomicU64,
    retransmitted: AtomicU64,
    naks: AtomicU64,
    lost: AtomicU64,
    dropped: AtomicU64,
    queue_dropped: AtomicU64,
    in_flight: AtomicU64,
    latency_ms: AtomicU32,
    bytes_sent: AtomicU64,
    packets_sent: AtomicU64,
    connections: AtomicU32,
}

impl Shared {
    fn wake(&self) {
        if self.parked.load(Ordering::Acquire) {
            self.waker.wake();
        }
    }
}

/// A running SRT caller. Dropping it closes the connection.
pub struct SrtSender {
    shared: Arc<Shared>,
    host_now_ns: Arc<dyn Fn() -> i64 + Send + Sync>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl SrtSender {
    /// Validates the config and starts the SRT thread, which connects (and
    /// reconnects, if configured). Events arrive on `observer`.
    pub fn start(
        config: CallerConfig,
        options: SenderOptions,
        observer: Arc<dyn SrtObserver>,
    ) -> Result<SrtSender, String> {
        config.validate()?;
        let shared = Arc::new(Shared {
            queue: SpscQueue::new(QUEUE_SLOTS),
            closed: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            parked: AtomicBool::new(false),
            waker: Waker::new().map_err(|e| format!("wake pipe: {e}"))?,
            rtt_us: AtomicU64::new(0),
            retransmitted: AtomicU64::new(0),
            naks: AtomicU64::new(0),
            lost: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            queue_dropped: AtomicU64::new(0),
            in_flight: AtomicU64::new(0),
            latency_ms: AtomicU32::new(0),
            bytes_sent: AtomicU64::new(0),
            packets_sent: AtomicU64::new(0),
            connections: AtomicU32::new(0),
        });
        let thread_shared = shared.clone();
        let now_ns = options.host_now_ns.clone();
        let thread = std::thread::Builder::new()
            .name("srt-sender".into())
            .spawn(move || {
                set_thread_qos();
                Worker {
                    shared: thread_shared,
                    config,
                    observer,
                    now_ns,
                }
                .run()
            })
            .map_err(|e| format!("thread: {e}"))?;
        Ok(SrtSender {
            shared,
            host_now_ns: options.host_now_ns,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Queues TS bytes (any length; split into 1316-byte payloads). With
    /// `frame_capture_ns` != 0 they are one video frame: its first and last
    /// payloads are reported through `on_frame_sent`. `now_ns` is the host
    /// time now (0 = read the clock here).
    ///
    /// No lock, one allocation per 64 payloads: safe on the encoder/muxer
    /// thread. Calls must be serialized (one producer). Returns the
    /// payloads queued.
    pub fn send(
        &self,
        data: &[u8],
        frame_capture_ns: i64,
        now_ns: i64,
    ) -> Result<usize, SendError> {
        let s = &self.shared;
        if !s.connected.load(Ordering::Acquire) {
            return Err(SendError::NotConnected);
        }
        let now_ns = if now_ns != 0 {
            now_ns
        } else {
            (self.host_now_ns)()
        };
        match s.queue.push(data, frame_capture_ns, now_ns) {
            Ok(n) => {
                s.wake();
                Ok(n)
            }
            Err(PushError::Full) => {
                s.queue_dropped.fetch_add(
                    data.len().div_ceil(crate::PAYLOAD_SIZE) as u64,
                    Ordering::Relaxed,
                );
                s.wake();
                Err(SendError::QueueFull)
            }
            Err(PushError::Busy) => Err(SendError::Busy),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.shared.connected.load(Ordering::Acquire)
    }

    pub fn stats(&self) -> SrtStats {
        let s = &self.shared;
        let r = |a: &AtomicU64| a.load(Ordering::Relaxed);
        SrtStats {
            connected: s.connected.load(Ordering::Relaxed),
            rtt_ms: r(&s.rtt_us) as f64 / 1000.0,
            retransmitted_packets: r(&s.retransmitted),
            received_naks: r(&s.naks),
            lost_packets: r(&s.lost),
            dropped_packets: r(&s.dropped) + r(&s.queue_dropped),
            packets_in_flight: r(&s.in_flight),
            negotiated_latency_ms: s.latency_ms.load(Ordering::Relaxed),
            bytes_sent: r(&s.bytes_sent),
            packets_sent: r(&s.packets_sent),
            connections: s.connections.load(Ordering::Relaxed),
        }
    }

    /// Sends a shutdown to the listener, stops the thread and waits for it.
    /// No events after this returns. Idempotent.
    pub fn close(&self) {
        self.shared.closed.store(true, Ordering::Release);
        self.shared.waker.wake();
        let thread = self.thread.lock().unwrap().take();
        if let Some(t) = thread {
            if t.thread().id() != std::thread::current().id() {
                let _ = t.join();
            }
        }
    }
}

impl Drop for SrtSender {
    fn drop(&mut self) {
        self.close();
    }
}

// ---------------------------------------------------------------------------
// SRT thread
// ---------------------------------------------------------------------------

struct Worker {
    shared: Arc<Shared>,
    config: CallerConfig,
    observer: Arc<dyn SrtObserver>,
    now_ns: Arc<dyn Fn() -> i64 + Send + Sync>,
}

enum End {
    Closed,
    Failed(String),
}

/// Per-connection state of the SRT thread.
struct SessionState {
    marks: VecDeque<FrameMark>,
    /// Sequence numbers the listener reported lost and that were not
    /// resent since.
    nak_wanted: HashSet<u32>,
    retransmits: u64,
    rto_suppressed: u64,
}

/// A payload that starts or ends a video frame, waiting to go out.
struct FrameMark {
    seq: SeqNumber,
    marks: u8,
    capture_ns: i64,
    enqueued_ns: i64,
}

impl Worker {
    fn run(self) {
        let mut attempt = 0;
        while !self.closed() {
            attempt += 1;
            self.observer.on_event(SrtEvent::Connecting { attempt });
            let end = match catch_unwind(AssertUnwindSafe(|| self.session())) {
                Ok(end) => end,
                Err(_) => End::Failed("SRT engine error".into()),
            };
            self.shared.connected.store(false, Ordering::Release);
            let dropped = self.shared.queue.clear();
            self.shared
                .dropped
                .fetch_add(dropped as u64, Ordering::Relaxed);
            match end {
                End::Closed => break,
                End::Failed(reason) => {
                    let again = self.config.reconnect && !self.closed();
                    info!("SRT disconnected: {reason}");
                    self.observer.on_event(SrtEvent::Disconnected {
                        reason,
                        reconnecting: again,
                    });
                    if !again {
                        break;
                    }
                    self.sleep(RECONNECT_DELAY, None);
                }
            }
        }
        self.shared.connected.store(false, Ordering::Release);
    }

    fn closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }

    /// Waits for `timeout`, the socket (if any) becoming readable, or a wake.
    fn sleep(&self, timeout: Duration, socket: Option<&UdpSocket>) {
        let s = &self.shared;
        s.parked.store(true, Ordering::SeqCst);
        // Payloads queued before `parked` was set came without a wake; one
        // after it wrote to the pipe, which a later poll drains.
        let idle = s.queue.is_empty() && !self.closed();
        let woken = if idle && !timeout.is_zero() {
            // Whole ms rounded up: poll(2) waits no shorter, and a wait cut
            // short only wakes the thread again. A sub-ms SND pacing wait
            // (0.2 ms per packet at the 50 Mbit/s cap) becomes 1 ms; the
            // engine then sends every period that elapsed, so the average
            // rate holds with one wakeup per ms instead of one per packet.
            let ms = timeout.as_nanos().div_ceil(1_000_000).min(1000) as i32;
            poll_waker(socket, &s.waker, ms)
        } else {
            true
        };
        s.parked.store(false, Ordering::SeqCst);
        if woken {
            s.waker.drain();
        }
    }

    fn session(&self) -> End {
        let cfg = &self.config;
        let remote = match (cfg.host.as_str(), cfg.port).to_socket_addrs() {
            Ok(mut addrs) => match addrs.next() {
                Some(a) => a,
                None => return End::Failed(format!("{}: no address", cfg.host)),
            },
            Err(e) => return End::Failed(format!("{}: {e}", cfg.host)),
        };
        let socket = match open_socket(remote) {
            Ok(s) => s,
            Err(e) => return End::Failed(format!("socket: {e}")),
        };
        let local_ip = socket.local_addr().map(|a| a.ip()).unwrap_or(match remote {
            SocketAddr::V4(_) => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            SocketAddr::V6(_) => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        });
        info!(
            "SRT connecting to {remote} streamid={} latency={} ms encryption={}",
            cfg.stream_id.as_deref().unwrap_or("-"),
            cfg.latency_ms,
            if cfg.passphrase.is_some() {
                "on"
            } else {
                "off"
            }
        );

        // Handshake.
        let mut connect = Connect::new(
            remote,
            local_ip,
            self.init_settings(),
            cfg.stream_id.clone(),
            rand::random(),
        );
        let start = Instant::now();
        let mut next_tick = start;
        let mut buf = [0u8; 2048];
        let mut socket_start: Option<Instant> = None;
        let connection = 'handshake: loop {
            if self.closed() {
                return End::Closed;
            }
            let now = Instant::now();
            if now - start > cfg.connect_timeout {
                return End::Failed("No answer from the SRT listener".into());
            }
            if now >= next_tick {
                next_tick = now + HANDSHAKE_REPEAT;
                if let ConnectionResult::SendPacket((mut p, _)) = connect.handle_tick(now) {
                    stamp_conclusion(&mut p, socket_start, now);
                    self.send_packet(&socket, &p);
                }
            }
            loop {
                let n = match socket.recv(&mut buf) {
                    Ok(n) => n,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    // Port unreachable: the listener may not be up yet.
                    Err(_) => break,
                };
                let Ok(packet) = Packet::parse(&mut &buf[..n], remote.is_ipv6()) else {
                    continue;
                };
                let now = Instant::now();
                match connect.handle_packet(Ok((packet, remote)), now) {
                    ConnectionResult::SendPacket((mut p, _)) => {
                        // srt-protocol starts the connection clock (the base
                        // of every data timestamp) when it builds the
                        // conclusion, with the `now` passed in here.
                        if is_conclusion(&p) && socket_start.is_none() {
                            socket_start = Some(now);
                        }
                        stamp_conclusion(&mut p, socket_start, now);
                        self.send_packet(&socket, &p)
                    }
                    ConnectionResult::Connected(p, c) => {
                        if let Some((p, _)) = p {
                            self.send_packet(&socket, &p);
                        }
                        break 'handshake c;
                    }
                    ConnectionResult::Reject(_, r) => return End::Failed(format!("{r}")),
                    ConnectionResult::Failure(e) => return End::Failed(format!("{e}")),
                    ConnectionResult::NotHandled(e) => debug!("SRT handshake: {e}"),
                    _ => {}
                }
            }
            self.sleep(
                next_tick.saturating_duration_since(Instant::now()),
                Some(&socket),
            );
        };

        let latency = connection.settings.send_tsbpd_latency;
        let start_time = connection.settings.socket_start_time;
        let remote_sockid = connection.settings.remote_sockid;
        let mut next_seq = connection.settings.init_seq_num;
        let mut conn = DuplexConnection::new(connection);
        // Counters of this connection are added to those of the earlier ones.
        let base = (
            self.shared.retransmitted.load(Ordering::Relaxed),
            self.shared.naks.load(Ordering::Relaxed),
            self.shared.lost.load(Ordering::Relaxed),
            self.shared.dropped.load(Ordering::Relaxed),
            self.shared.packets_sent.load(Ordering::Relaxed),
        );
        self.shared
            .latency_ms
            .store(latency.as_millis() as u32, Ordering::Relaxed);
        self.shared.connections.fetch_add(1, Ordering::Relaxed);
        self.shared.queue.clear();
        self.shared.connected.store(true, Ordering::Release);
        info!(
            "SRT connected to {remote}, latency {} ms (asked {} ms)",
            latency.as_millis(),
            cfg.latency_ms
        );
        self.observer.on_event(SrtEvent::Connected {
            latency_ms: latency.as_millis() as u32,
            peer: remote.to_string(),
        });

        let mut st = SessionState {
            marks: VecDeque::with_capacity(64),
            nak_wanted: HashSet::with_capacity(256),
            retransmits: 0,
            rto_suppressed: 0,
        };
        let mut peer_shutdown = false;
        let mut unreachable_since: Option<Instant> = None;
        let mut last_heard = Instant::now();
        // When the engine's timers last ran (`now` of the last timer input).
        let mut timers_at = last_heard;
        loop {
            if self.closed() {
                let shutdown = Packet::Control(ControlPacket {
                    timestamp: TimeStamp::from_micros(start_time.elapsed().as_micros() as u32),
                    dest_sockid: remote_sockid,
                    control_type: ControlTypes::Shutdown,
                });
                self.send_packet(&socket, &shutdown);
                debug!(
                    "SRT closed; {} timeout retransmits not sent",
                    st.rto_suppressed
                );
                return End::Closed;
            }
            let now = Instant::now();
            let mut open = true;

            // Payloads from the producer, first and at the time the engine's
            // timers last ran. Every input runs the timers, and the SND
            // timer releases the packets due since its last run: fed at
            // `now`, the first payload would take all of them and the rest
            // of the frame would go out one SND period (about 10 us without
            // a bandwidth cap) at a time, one sleep and wakeup per packet.
            // This way the timer run below sends the whole burst at once.
            while let Some(slot) = self.shared.queue.front() {
                let src = self.source_time(slot.enqueued_ns, now, start_time);
                if slot.marks != 0 {
                    st.marks.push_back(FrameMark {
                        seq: next_seq,
                        marks: slot.marks,
                        capture_ns: slot.capture_ns,
                        enqueued_ns: slot.enqueued_ns,
                    });
                }
                next_seq += 1;
                let data = slot.data.clone();
                self.shared.queue.pop();
                open &= self
                    .pump(
                        &mut st,
                        &mut conn,
                        &socket,
                        timers_at,
                        Input::Data(Some((src, data))),
                    )
                    .is_some();
            }

            // Packets from the listener.
            loop {
                let n = match socket.recv(&mut buf) {
                    Ok(n) => n,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::ConnectionRefused => {
                        unreachable_since.get_or_insert(now);
                        continue;
                    }
                    Err(e) => {
                        debug!("SRT receive: {e}");
                        break;
                    }
                };
                let packet = match Packet::parse(&mut &buf[..n], remote.is_ipv6()) {
                    Ok(p) => p,
                    Err(e) => {
                        debug!("SRT parse: {e:?}");
                        continue;
                    }
                };
                last_heard = now;
                unreachable_since = None;
                if let Packet::Control(c) = &packet {
                    match &c.control_type {
                        ControlTypes::Ack(
                            Acknowledgement::Full(_, s, _) | Acknowledgement::Small(_, s),
                        ) => {
                            let rtt = s.rtt.mean().as_micros().max(0) as u64;
                            self.shared.rtt_us.store(rtt, Ordering::Relaxed);
                        }
                        ControlTypes::Shutdown => peer_shutdown = true,
                        ControlTypes::Nak(list) => {
                            if st.nak_wanted.len() > 16384 {
                                st.nak_wanted.clear();
                            }
                            let lost = list.clone().into_iter_decompressed().take(8192);
                            st.nak_wanted.extend(lost.map(|n| n.as_raw()));
                        }
                        // The engine would panic on these (todo!/unimplemented!).
                        ControlTypes::CongestionWarning | ControlTypes::PeerError(_) => continue,
                        ControlTypes::Srt(
                            SrtControlPacket::KeyRefreshRequest(_)
                            | SrtControlPacket::KeyRefreshResponse(_)
                            | SrtControlPacket::HandshakeRequest(_)
                            | SrtControlPacket::HandshakeResponse(_),
                        ) => {}
                        ControlTypes::Srt(_) => continue,
                        _ => {}
                    }
                }
                open &= self
                    .pump(
                        &mut st,
                        &mut conn,
                        &socket,
                        now,
                        Input::Packet(Ok((packet, remote))),
                    )
                    .is_some();
            }

            let wait = self.pump(&mut st, &mut conn, &socket, now, Input::Timer);
            timers_at = now;
            self.publish(&conn, &st, base);
            let wait = match (open, wait) {
                (true, Some(w)) => w,
                _ => {
                    return End::Failed(if peer_shutdown {
                        "The listener closed the connection".into()
                    } else {
                        format!(
                            "Connection lost (nothing heard for {} s)",
                            cfg.peer_idle_timeout.as_secs()
                        )
                    })
                }
            };
            if let Some(t) = unreachable_since {
                if now - t > UNREACHABLE_GRACE && now - last_heard > UNREACHABLE_GRACE {
                    return End::Failed("Connection lost (listener port unreachable)".into());
                }
            }
            self.sleep(wait, Some(&socket));
        }
    }

    fn init_settings(&self) -> ConnInitSettings {
        let cfg = &self.config;
        let latency = Duration::from_millis(cfg.latency_ms.into());
        let key_settings = cfg.passphrase.as_ref().map(|p| KeySettings {
            key_size: match cfg.pbkeylen {
                24 => KeySize::AES192,
                32 => KeySize::AES256,
                _ => KeySize::AES128,
            },
            passphrase: Passphrase::from(p.as_str()),
        });
        let bandwidth = if cfg.max_bandwidth == 0 {
            LiveBandwidthMode::Unlimited
        } else {
            LiveBandwidthMode::Max(DataRate(cfg.max_bandwidth))
        };
        ConnInitSettings {
            local_sockid: rand::random(),
            key_settings,
            key_refresh: KeyMaterialRefreshSettings::new(1 << 24, 1 << 12)
                .expect("libsrt km defaults"),
            send_latency: latency,
            recv_latency: latency,
            peer_idle_timeout: cfg.peer_idle_timeout,
            bandwidth,
            statistics_interval: Duration::from_secs(1),
            too_late_packet_drop: true,
            recv_buffer_size: PacketCount(8192),
            send_buffer_size: PacketCount(SEND_BUFFER_PACKETS.into()),
            max_packet_size: PacketSize(1500),
            max_flow_size: PacketCount(25600),
        }
    }

    /// The source time srt-protocol stamps a payload with (its TSBPD
    /// timestamp): when the producer queued it, not when this thread woke.
    fn source_time(&self, enqueued_ns: i64, now: Instant, start: Instant) -> Instant {
        let age = ((self.now_ns)() - enqueued_ns).max(0) as u64;
        now.checked_sub(Duration::from_nanos(age))
            .map_or(start, |t| t.max(start))
    }

    /// Feeds one input and runs the engine until it waits: the time until
    /// its next timer, or None when it closed the connection.
    fn pump(
        &self,
        st: &mut SessionState,
        conn: &mut DuplexConnection,
        socket: &UdpSocket,
        now: Instant,
        input: Input,
    ) -> Option<Duration> {
        let mut input = input;
        loop {
            match conn.handle_input(now, input) {
                Action::SendPacket((p, _)) => {
                    input = Input::PacketSent;
                    if let Packet::Data(d) = &p {
                        if !d.retransmitted {
                            self.frame_sent(d.seq_number, &mut st.marks);
                        } else if st.nak_wanted.remove(&d.seq_number.as_raw()) {
                            st.retransmits += 1;
                        } else {
                            // srt-protocol also resends on its own timeout
                            // (RTO). After one loss the ACK stalls and every
                            // later packet times out: at 2% loss that was a
                            // third more packets on the wire. libsrt in live
                            // mode resends only what the receiver reports
                            // lost (and its periodic NAK report repeats a
                            // loss), so do the same.
                            st.rto_suppressed += 1;
                            continue;
                        }
                    }
                    self.send_packet(socket, &p);
                }
                Action::ReleaseData(_) => input = Input::DataReleased,
                Action::UpdateStatistics(_) => input = Input::StatisticsUpdated,
                Action::WaitForData(w) => return Some(w),
                Action::Close => return None,
            }
        }
    }

    fn frame_sent(&self, seq: SeqNumber, marks: &mut VecDeque<FrameMark>) {
        while let Some(m) = marks.front() {
            if m.seq > seq {
                return;
            }
            let m = marks.pop_front().unwrap();
            if m.seq < seq {
                // Dropped before it went out.
                continue;
            }
            let sent_ns = (self.now_ns)();
            if m.marks & MARK_FRAME_START != 0 {
                self.observer
                    .on_frame_sent(m.capture_ns, m.enqueued_ns, sent_ns, false);
            }
            if m.marks & MARK_FRAME_END != 0 {
                self.observer
                    .on_frame_sent(m.capture_ns, m.enqueued_ns, sent_ns, true);
            }
            return;
        }
    }

    fn send_packet(&self, socket: &UdpSocket, packet: &Packet) {
        let mut out = [0u8; 1600];
        let mut w = &mut out[..];
        packet.serialize(&mut w);
        let len = 1600 - w.len();
        match socket.send(&out[..len]) {
            Ok(_) => {
                self.shared
                    .bytes_sent
                    .fetch_add(len as u64 + IP_UDP_HEADER, Ordering::Relaxed);
            }
            // Lost like on the wire; SRT retransmits.
            Err(e) => debug!("SRT send: {e}"),
        }
    }

    fn publish(
        &self,
        conn: &DuplexConnection,
        state: &SessionState,
        base: (u64, u64, u64, u64, u64),
    ) {
        let st = conn.statistics();
        let s = &self.shared;
        s.retransmitted
            .store(base.0 + state.retransmits, Ordering::Relaxed);
        s.naks.store(base.1 + st.rx_nak, Ordering::Relaxed);
        s.lost.store(base.2 + st.tx_loss_data, Ordering::Relaxed);
        s.dropped
            .store(base.3 + st.tx_dropped_data, Ordering::Relaxed);
        s.packets_sent
            .store(base.4 + st.tx_unique_data, Ordering::Relaxed);
        // Refreshed with the engine's statistics interval (1 s).
        s.in_flight.store(st.tx_buffered_data, Ordering::Relaxed);
    }
}

fn is_conclusion(p: &Packet) -> bool {
    matches!(
        p,
        Packet::Control(ControlPacket {
            control_type: ControlTypes::Handshake(HandshakeControlInfo {
                shake_type: ShakeType::Conclusion,
                ..
            }),
            ..
        })
    )
}

/// srt-protocol 0.4.4 sends the conclusion handshake with the timestamp of
/// the listener's induction answer (the listener's clock). libsrt takes the
/// conclusion's timestamp as the caller's clock to derive the peer start
/// time for TSBPD, so every data packet then looks hundreds of ms late:
/// played out at once (no latency buffer) and lost packets skipped instead
/// of waited for. Stamp it with our connection clock, as libsrt does.
fn stamp_conclusion(p: &mut Packet, socket_start: Option<Instant>, now: Instant) {
    if let (Packet::Control(c), Some(start)) = (&mut *p, socket_start) {
        if matches!(&c.control_type, ControlTypes::Handshake(h) if h.shake_type == ShakeType::Conclusion)
        {
            c.timestamp =
                TimeStamp::from_micros(now.saturating_duration_since(start).as_micros() as u32);
        }
    }
}

// ---------------------------------------------------------------------------
// Platform
// ---------------------------------------------------------------------------

fn open_socket(remote: SocketAddr) -> io::Result<UdpSocket> {
    let local: SocketAddr = match remote {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(local)?;
    socket.connect(remote)?;
    socket.set_nonblocking(true)?;
    let fd = socket.as_raw_fd();
    // A keyframe leaves in one burst; the default UDP send buffer (9 KB on
    // Darwin) would drop most of it.
    set_int_opt(fd, libc::SOL_SOCKET, libc::SO_SNDBUF, 2 << 20);
    set_int_opt(fd, libc::SOL_SOCKET, libc::SO_RCVBUF, 256 << 10);
    #[cfg(target_vendor = "apple")]
    {
        // SO_NET_SERVICE_TYPE = NET_SERVICE_TYPE_VI (interactive video), what
        // NWParameters.serviceClass = .interactiveVideo sets.
        set_int_opt(fd, libc::SOL_SOCKET, 0x1116, 3);
    }
    Ok(socket)
}

fn set_int_opt(fd: i32, level: i32, name: i32, value: i32) {
    // SAFETY: valid fd, pointer to a live i32 of the given length.
    let r = unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            &value as *const i32 as *const libc::c_void,
            std::mem::size_of::<i32>() as libc::socklen_t,
        )
    };
    if r != 0 {
        warn!("setsockopt {level}/{name}: {}", io::Error::last_os_error());
    }
}

fn set_thread_qos() {
    #[cfg(target_vendor = "apple")]
    // SAFETY: plain call on the current thread.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

/// Waits up to `timeout_ms` for the socket to become readable or a wake;
/// true when the wake pipe has bytes to drain.
fn poll_waker(socket: Option<&UdpSocket>, waker: &Waker, timeout_ms: i32) -> bool {
    let mut fds = [
        libc::pollfd {
            fd: waker.read_fd,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: socket.map_or(-1, |s| s.as_raw_fd()),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    // SAFETY: two valid pollfd entries (fd -1 is ignored by poll).
    let r = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout_ms) };
    r != 0 && (r < 0 || fds[0].revents != 0)
}

/// A non-blocking pipe that wakes the SRT thread out of poll(2).
struct Waker {
    read_fd: i32,
    write_fd: i32,
}

impl Waker {
    fn new() -> io::Result<Waker> {
        let mut fds = [0i32; 2];
        // SAFETY: fds has room for two descriptors.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        for fd in fds {
            // SAFETY: fds just created.
            unsafe {
                libc::fcntl(
                    fd,
                    libc::F_SETFL,
                    libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK,
                );
                libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            }
        }
        Ok(Waker {
            read_fd: fds[0],
            write_fd: fds[1],
        })
    }

    fn wake(&self) {
        let b = 1u8;
        // SAFETY: valid fd; a full pipe (EAGAIN) already wakes the reader.
        unsafe { libc::write(self.write_fd, &b as *const u8 as *const libc::c_void, 1) };
    }

    fn drain(&self) {
        let mut buf = [0u8; 64];
        // SAFETY: valid fd and buffer.
        while unsafe {
            libc::read(
                self.read_fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        } > 0
        {}
    }
}

impl Drop for Waker {
    fn drop(&mut self) {
        // SAFETY: owned fds.
        unsafe {
            libc::close(self.read_fd);
            libc::close(self.write_fd);
        }
    }
}
