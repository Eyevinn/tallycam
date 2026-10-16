//! std platform driver (feature `driver`): runs a clock source on its own
//! thread with UDP sockets, blocking DNS on helper threads and the host
//! clock of the platform, and publishes the mapping through a
//! [`SnapshotCell`] so hot-path readers never block.
//!
//! The platform pieces are traits ([`HostClock`], [`Resolver`],
//! [`LogSink`]) so another platform can supply its own.

use std::io::Write as _;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::protocol::ClockStatus;

use super::config::ClockConfig;
use super::engine::{Action, Engine, PathInfo, RandomSource, SourceId};
use super::log::LogKind;
use super::mapping::{MappingSnapshot, SnapshotCell};
use super::report;
use super::system::{SystemClock, SystemClockReading};
use super::ClockSource;

// ---------------------------------------------------------------------------
// Platform traits
// ---------------------------------------------------------------------------

/// The host clock: the clock media capture timestamps are taken in.
pub trait HostClock: Send + Sync + 'static {
    /// Monotonic ns.
    fn now_ns(&self) -> i64;
    /// Total time the host clock did not count (device sleep), ns. Zero for
    /// clocks that count sleep.
    fn paused_ns(&self) -> i64 {
        0
    }
    /// The platform wall clock (UTC), ns since the Unix epoch.
    fn wall_utc_ns(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as i64)
            .unwrap_or(0)
    }
}

/// `mach_absolute_time` in ns: the clock of `CMClockGetHostTimeClock` and of
/// camera and audio sample buffer timestamps on Apple platforms. It stops
/// while the device sleeps; `mach_continuous_time` does not, and the
/// difference is the sleep time.
#[cfg(target_vendor = "apple")]
pub struct MachHostClock {
    numer: u64,
    denom: u64,
}

#[cfg(target_vendor = "apple")]
#[repr(C)]
struct MachTimebaseInfo {
    numer: u32,
    denom: u32,
}

#[cfg(target_vendor = "apple")]
extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_continuous_time() -> u64;
    fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
}

#[cfg(target_vendor = "apple")]
impl Default for MachHostClock {
    fn default() -> Self {
        let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: plain out-parameter.
        unsafe { mach_timebase_info(&mut info) };
        MachHostClock {
            numer: info.numer.max(1) as u64,
            denom: info.denom.max(1) as u64,
        }
    }
}

#[cfg(target_vendor = "apple")]
impl MachHostClock {
    /// Host ticks (`mach_absolute_time` units) to ns, without overflow.
    #[inline]
    pub fn ticks_to_ns(&self, ticks: u64) -> i64 {
        if self.numer == self.denom {
            return ticks as i64;
        }
        (ticks / self.denom * self.numer + ticks % self.denom * self.numer / self.denom) as i64
    }
}

#[cfg(target_vendor = "apple")]
impl HostClock for MachHostClock {
    fn now_ns(&self) -> i64 {
        // SAFETY: no arguments, no side effects.
        self.ticks_to_ns(unsafe { mach_absolute_time() })
    }

    fn paused_ns(&self) -> i64 {
        // SAFETY: as above.
        let (c, a) = unsafe { (mach_continuous_time(), mach_absolute_time()) };
        self.ticks_to_ns(c.saturating_sub(a))
    }
}

/// `CLOCK_MONOTONIC` through `std::time::Instant`, for platforms without a
/// better host clock (it may or may not count suspend).
pub struct InstantHostClock {
    origin: std::time::Instant,
}

impl Default for InstantHostClock {
    fn default() -> Self {
        InstantHostClock {
            origin: std::time::Instant::now(),
        }
    }
}

impl HostClock for InstantHostClock {
    fn now_ns(&self) -> i64 {
        self.origin.elapsed().as_nanos() as i64 + 1_000_000_000
    }
}

/// `CLOCK_BOOTTIME` in ns: the clock of Camera2 `SENSOR_TIMESTAMP` with the
/// `REALTIME` timestamp source, of `SystemClock.elapsedRealtimeNanos()` and
/// of `AudioTimestamp.TIMEBASE_BOOTTIME`. It counts suspend, so nothing is
/// paused.
#[cfg(target_os = "android")]
#[derive(Debug, Default)]
pub struct BootTimeHostClock;

#[cfg(target_os = "android")]
impl HostClock for BootTimeHostClock {
    fn now_ns(&self) -> i64 {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: plain out-parameter; CLOCK_BOOTTIME exists on every Android.
        unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
        // time_t and c_long are i64 on 64-bit Android, i32 on 32-bit ARM.
        #[allow(clippy::unnecessary_cast)]
        let ns = ts.tv_sec as i64 * 1_000_000_000 + ts.tv_nsec as i64;
        ns
    }
}

/// The best host clock of this platform.
pub fn default_host_clock() -> Arc<dyn HostClock> {
    #[cfg(target_vendor = "apple")]
    {
        Arc::new(MachHostClock::default())
    }
    #[cfg(target_os = "android")]
    {
        Arc::new(BootTimeHostClock)
    }
    #[cfg(not(any(target_vendor = "apple", target_os = "android")))]
    {
        Arc::new(InstantHostClock::default())
    }
}

/// Name resolution (IPv4 and IPv6).
pub trait Resolver: Send + Sync + 'static {
    fn resolve(&self, name: &str) -> Result<Vec<IpAddr>, String>;
}

/// `getaddrinfo` through std.
pub struct StdResolver;

impl Resolver for StdResolver {
    fn resolve(&self, name: &str) -> Result<Vec<IpAddr>, String> {
        let mut out: Vec<IpAddr> = Vec::new();
        for a in (name, 123u16)
            .to_socket_addrs()
            .map_err(|e| e.to_string())?
        {
            if !out.contains(&a.ip()) {
                out.push(a.ip());
            }
        }
        Ok(out)
    }
}

/// Where log records go besides the in-memory ring.
pub trait LogSink: Send {
    fn write(&mut self, kind: LogKind, line: &str);
}

/// Appends records to a file, one `kind line` per line; rotates to `.1`
/// at `max_bytes`.
pub struct FileLogSink {
    path: PathBuf,
    file: Option<std::fs::File>,
    written: u64,
    max_bytes: u64,
}

impl FileLogSink {
    pub fn new(path: PathBuf, max_bytes: u64) -> Self {
        FileLogSink {
            path,
            file: None,
            written: 0,
            max_bytes,
        }
    }
}

impl LogSink for FileLogSink {
    fn write(&mut self, kind: LogKind, line: &str) {
        if self.file.is_none() || self.written > self.max_bytes {
            if self.written > self.max_bytes {
                let mut old = self.path.clone().into_os_string();
                old.push(".1");
                let _ = std::fs::rename(&self.path, old);
            }
            if let Some(dir) = self.path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            self.file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)
                .ok();
            self.written = self
                .file
                .as_ref()
                .and_then(|f| f.metadata().ok())
                .map_or(0, |m| m.len());
        }
        if let Some(f) = self.file.as_mut() {
            let _ = writeln!(f, "{} {}", kind.name(), line);
            self.written += line.len() as u64 + 16;
        }
    }
}

/// Randomness from the OS.
pub struct OsRandom;

impl RandomSource for OsRandom {
    fn next_u64(&mut self) -> u64 {
        os_random_u64()
    }
}

#[cfg(target_vendor = "apple")]
extern "C" {
    fn arc4random_buf(buf: *mut core::ffi::c_void, n: usize);
}

pub fn os_random_u64() -> u64 {
    let mut b = [0u8; 8];
    #[cfg(target_vendor = "apple")]
    // SAFETY: writes exactly 8 bytes into `b`.
    unsafe {
        arc4random_buf(b.as_mut_ptr().cast(), 8)
    };
    #[cfg(not(target_vendor = "apple"))]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let _ = f.read_exact(&mut b);
        }
    }
    u64::from_ne_bytes(b)
}

// ---------------------------------------------------------------------------
// Discipline driver
// ---------------------------------------------------------------------------

/// Longest sleep of the driver thread: also how quickly it notices device
/// sleep, live changes and path bursts.
const MAX_WAIT: Duration = Duration::from_millis(250);
/// Requests whose reply did not come within this are closed.
const SOCKET_TTL_NS: i64 = 8_000_000_000;
/// Host clock pauses shorter than this are not sleep.
const SLEEP_THRESHOLD_NS: i64 = 50_000_000;

struct Pending {
    source: SourceId,
    sock: UdpSocket,
    opened: i64,
}

/// Optional platform storage for the driver.
#[derive(Default)]
pub struct DriverOptions {
    /// Log records also go here.
    pub log_sink: Option<Box<dyn LogSink>>,
    /// chrony-style driftfile: "<freq ppm> <skew ppm>", read at start and
    /// written hourly and on stop.
    pub drift_file: Option<PathBuf>,
}

/// Reads a driftfile.
pub fn read_drift_file(path: &std::path::Path) -> Option<(f64, f64)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut it = text.split_whitespace().map(|v| v.parse::<f64>());
    match (it.next(), it.next()) {
        (Some(Ok(f)), Some(Ok(s))) if f.is_finite() && s.is_finite() && f.abs() < 2000.0 => {
            Some((f, s))
        }
        _ => None,
    }
}

fn write_drift_file(path: &std::path::Path, drift: (f64, f64)) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, format!("{:.6} {:.6}\n", drift.0, drift.1)).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// How often the driftfile is written (chrony: hourly).
const DRIFT_FILE_INTERVAL_NS: i64 = 3600 * 1_000_000_000;

struct Shared {
    engine: Mutex<Engine>,
    cell: Arc<SnapshotCell>,
    host: Arc<dyn HostClock>,
    resolver: Arc<dyn Resolver>,
    sink: Mutex<Option<Box<dyn LogSink>>>,
    drift_file: Option<PathBuf>,
    stop: AtomicBool,
}

impl Shared {
    fn save_drift(&self, engine: &Engine) {
        if let (Some(path), Some(drift)) = (&self.drift_file, engine.drift()) {
            if drift.1 < 100.0 {
                write_drift_file(path, drift);
            }
        }
    }

    fn publish(&self, engine: &Engine, now: i64) {
        self.cell.store(&engine.snapshot(now));
    }
}

/// The built-in discipline on a thread of its own.
pub struct DisciplineDriver {
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl DisciplineDriver {
    /// Starts at once. `cell` receives every new mapping.
    pub fn start(
        mut cfg: ClockConfig,
        host: Arc<dyn HostClock>,
        resolver: Arc<dyn Resolver>,
        options: DriverOptions,
        cell: Arc<SnapshotCell>,
    ) -> Arc<Self> {
        if cfg.drift.is_none() {
            cfg.drift = options.drift_file.as_deref().and_then(read_drift_file);
        }
        let now = host.now_ns();
        let engine = Engine::new(cfg, now, host.wall_utc_ns(), Box::new(OsRandom));
        cell.store(&engine.snapshot(now));
        let shared = Arc::new(Shared {
            engine: Mutex::new(engine),
            cell,
            host,
            resolver,
            sink: Mutex::new(options.log_sink),
            drift_file: options.drift_file,
            stop: AtomicBool::new(false),
        });
        let thread_shared = shared.clone();
        let handle = std::thread::Builder::new()
            .name("clock-discipline".into())
            .spawn(move || run(thread_shared))
            .expect("spawn clock thread");
        Arc::new(DisciplineDriver {
            shared,
            thread: Mutex::new(Some(handle)),
        })
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.thread.lock().unwrap().take() {
            let _ = h.join();
            self.shared.save_drift(&self.shared.engine.lock().unwrap());
        }
    }

    pub fn host(&self) -> &Arc<dyn HostClock> {
        &self.shared.host
    }

    /// Runs `f` on the engine (reports, tests).
    pub fn with_engine<R>(&self, f: impl FnOnce(&mut Engine, i64) -> R) -> R {
        let mut e = self.shared.engine.lock().unwrap();
        let now = self.shared.host.now_ns();
        let r = f(&mut e, now);
        self.shared.publish(&e, now);
        r
    }
}

impl Drop for DisciplineDriver {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

fn open_socket(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let bind: SocketAddr = if addr.is_ipv4() {
        "0.0.0.0:0".parse().unwrap()
    } else {
        "[::]:0".parse().unwrap()
    };
    let sock = UdpSocket::bind(bind)?;
    sock.connect(addr)?;
    sock.set_nonblocking(true)?;
    Ok(sock)
}

fn run(shared: Arc<Shared>) {
    let mut pending: Vec<Pending> = Vec::new();
    let mut last_paused = shared.host.paused_ns();
    let mut log_seq = 0u64;
    let mut buf = [0u8; 1024];
    let mut drift_saved: Option<i64> = None;
    while !shared.stop.load(Ordering::Relaxed) {
        let wait_ns;
        {
            let mut engine = shared.engine.lock().unwrap();
            let now = shared.host.now_ns();
            let paused = shared.host.paused_ns();
            if paused - last_paused > SLEEP_THRESHOLD_NS {
                engine.on_host_paused(now, paused - last_paused);
                pending.clear();
            }
            last_paused = paused;

            for action in engine.poll(now) {
                match action {
                    Action::Send {
                        source,
                        addr,
                        packet,
                    } => {
                        pending.retain(|p| p.source != source);
                        let Ok(sock) = open_socket(addr) else {
                            continue;
                        };
                        let t1 = shared.host.now_ns();
                        if sock.send(&packet).is_ok() {
                            engine.on_transmitted(source, &packet, t1);
                            pending.push(Pending {
                                source,
                                sock,
                                opened: t1,
                            });
                        }
                    }
                    Action::Resolve { config_index, name } => {
                        let s = shared.clone();
                        let _ =
                            std::thread::Builder::new()
                                .name("clock-dns".into())
                                .spawn(move || {
                                    let result = s.resolver.resolve(&name);
                                    let mut e = s.engine.lock().unwrap();
                                    let now = s.host.now_ns();
                                    e.on_resolved(now, config_index, result);
                                });
                    }
                }
            }
            let now = shared.host.now_ns();
            pending.retain(|p| now - p.opened < SOCKET_TTL_NS);
            shared.publish(&engine, now);
            // First save once the frequency is known (skew under 100 ppm), then hourly.
            if engine.drift().is_some_and(|(_, skew)| skew < 100.0)
                && drift_saved.is_none_or(|t| now - t >= DRIFT_FILE_INTERVAL_NS)
                && engine.is_synchronised(now)
            {
                shared.save_drift(&engine);
                drift_saved = Some(now);
            }
            if let Some(sink) = shared.sink.lock().unwrap().as_mut() {
                for r in engine.log().since(log_seq) {
                    sink.write(r.kind, &r.line);
                    log_seq = r.seq;
                }
            }
            wait_ns = (engine.next_wake(now) - now).clamp(0, MAX_WAIT.as_nanos() as i64);
        }

        // Wait for replies or the next timer.
        let ready = wait_readable(&pending, wait_ns);
        // t4: as early as possible after the wakeup.
        let t4 = shared.host.now_ns();
        if ready.is_empty() {
            continue;
        }
        let mut engine = shared.engine.lock().unwrap();
        let mut done = Vec::new();
        for idx in ready {
            let p = &pending[idx];
            while let Ok(n) = p.sock.recv(&mut buf) {
                engine.on_packet(t4, p.source, &buf[..n]);
                done.push(p.source);
            }
        }
        pending.retain(|p| !done.contains(&p.source));
        shared.publish(&engine, t4);
    }
}

/// Indices of the sockets with data, waiting at most `wait_ns`.
fn wait_readable(pending: &[Pending], wait_ns: i64) -> Vec<usize> {
    use std::os::fd::AsRawFd;
    let timeout_ms = ((wait_ns + 999_999) / 1_000_000) as i32;
    if pending.is_empty() {
        std::thread::sleep(Duration::from_nanos(wait_ns.max(0) as u64));
        return Vec::new();
    }
    let mut fds: Vec<libc::pollfd> = pending
        .iter()
        .map(|p| libc::pollfd {
            fd: p.sock.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    // SAFETY: `fds` is a valid array of `fds.len()` pollfd structs.
    let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
    if n <= 0 {
        return Vec::new();
    }
    fds.iter()
        .enumerate()
        .filter(|(_, f)| f.revents & libc::POLLIN != 0)
        .map(|(i, _)| i)
        .collect()
}

impl ClockSource for DisciplineDriver {
    fn snapshot(&self) -> Option<MappingSnapshot> {
        self.shared.cell.load()
    }

    fn status(&self) -> ClockStatus {
        let e = self.shared.engine.lock().unwrap();
        let now = self.shared.host.now_ns();
        let t = e.tracking_report(now);
        let s = e.sources_report(now);
        let utc = e.snapshot(now).utc_ns(now);
        let device = (utc - self.shared.host.wall_utc_ns()) as f64 / 1e6;
        report::clock_status(&t, &s, Some(device))
    }

    fn reports_text(&self) -> String {
        let e = self.shared.engine.lock().unwrap();
        let now = self.shared.host.now_ns();
        let s = e.sources_report(now);
        format!(
            "{}\n{}\n{}",
            report::format_sources(&s),
            report::format_sourcestats(&s),
            report::format_tracking(&e.tracking_report(now))
        )
    }

    fn log_tail(&self, kinds: &[LogKind], limit: usize) -> Vec<String> {
        self.shared.engine.lock().unwrap().log().tail(kinds, limit)
    }

    fn set_live(&self, live: bool) {
        self.with_engine(|e, now| e.set_live(now, live));
    }

    fn path_changed(&self, path: PathInfo) {
        self.with_engine(|e, now| e.on_path_change(now, path));
    }
}

// ---------------------------------------------------------------------------
// System clock driver
// ---------------------------------------------------------------------------

/// Reads the OS clock and its kernel discipline state (read only).
pub fn read_system_clock(host: &dyn HostClock) -> SystemClockReading {
    let host_ns = host.now_ns();
    let utc_ns = host.wall_utc_ns();
    // Filled in below where the OS reports its discipline state.
    #[cfg_attr(not(target_os = "linux"), allow(unused_mut))]
    let mut r = SystemClockReading {
        host_ns,
        utc_ns,
        state: "unknown".into(),
        ..Default::default()
    };
    #[cfg(target_os = "linux")]
    {
        // SAFETY: zeroed timex with modes 0 is a read-only query.
        let mut tx: libc::timex = unsafe { std::mem::zeroed() };
        let state = unsafe { libc::ntp_adjtime(&mut tx) };
        if state >= 0 {
            r.state = match state {
                libc::TIME_OK => "ok",
                libc::TIME_INS => "ins",
                libc::TIME_DEL => "del",
                libc::TIME_OOP => "oop",
                libc::TIME_WAIT => "wait",
                libc::TIME_ERROR => "error",
                _ => "unknown",
            }
            .into();
            r.synchronized = Some(tx.status & libc::STA_UNSYNC == 0);
            r.offset_ns = if tx.status & libc::STA_NANO != 0 {
                tx.offset as i64
            } else {
                (tx.offset as i64) * 1000
            };
            r.frequency_ppm = tx.freq as f64 / 65_536.0;
            r.max_error_us = tx.maxerror as i64;
            r.est_error_us = tx.esterror as i64;
            r.tai_offset_sec = tx.tai;
        }
    }
    #[cfg(target_vendor = "apple")]
    {
        // SAFETY: plain out-parameter.
        let mut tv: libc::ntptimeval = unsafe { std::mem::zeroed() };
        let state = unsafe { libc::ntp_gettime(&mut tv) };
        if state >= 0 {
            // macOS keeps the kernel NTP state but timed does not maintain
            // it like chrony does on Linux: report what is there.
            r.state = if state == 5 {
                "error".into()
            } else {
                "ok".into()
            };
            r.synchronized = Some(state != 5);
            r.max_error_us = tv.maxerror as i64;
            r.est_error_us = tv.esterror as i64;
            r.tai_offset_sec = tv.tai as i32;
        }
    }
    r
}

struct SystemShared {
    clock: Mutex<SystemClock>,
    cell: Arc<SnapshotCell>,
    host: Arc<dyn HostClock>,
    live: AtomicBool,
    stop: AtomicBool,
}

/// The OS clock as clock source, for hosts that run chrony themselves.
pub struct SystemClockDriver {
    shared: Arc<SystemShared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl SystemClockDriver {
    pub fn start(host: Arc<dyn HostClock>, cell: Arc<SnapshotCell>) -> Arc<Self> {
        let shared = Arc::new(SystemShared {
            clock: Mutex::new(SystemClock::new()),
            cell,
            host,
            live: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        });
        let tick = |s: &SystemShared| {
            let r = read_system_clock(s.host.as_ref());
            let mut c = s.clock.lock().unwrap();
            c.update(r, s.live.load(Ordering::Relaxed));
            if let Some(snap) = c.snapshot() {
                s.cell.store(&snap);
            }
        };
        tick(&shared);
        let s = shared.clone();
        let handle = std::thread::Builder::new()
            .name("clock-system".into())
            .spawn(move || {
                while !s.stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_secs(1));
                    tick(&s);
                }
            })
            .expect("spawn clock thread");
        Arc::new(SystemClockDriver {
            shared,
            thread: Mutex::new(Some(handle)),
        })
    }

    pub fn stop(&self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.thread.lock().unwrap().take() {
            let _ = h.join();
        }
    }
}

impl Drop for SystemClockDriver {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

impl ClockSource for SystemClockDriver {
    fn snapshot(&self) -> Option<MappingSnapshot> {
        self.shared.cell.load()
    }

    fn status(&self) -> ClockStatus {
        self.shared.clock.lock().unwrap().status()
    }

    fn reports_text(&self) -> String {
        let st = self.status();
        format!(
            "System clock: synchronised {} health {:?} max error {} us est error {} us offset {} ns TAI-UTC {} s\n{}\n",
            st.synced,
            st.health,
            st.max_error_us,
            st.est_error_us,
            st.system_offset_ns,
            st.tai_utc_s,
            st.findings.join("\n")
        )
    }

    fn log_tail(&self, _kinds: &[LogKind], _limit: usize) -> Vec<String> {
        Vec::new()
    }

    fn set_live(&self, live: bool) {
        self.shared.live.store(live, Ordering::Relaxed);
    }

    fn path_changed(&self, _path: PathInfo) {}
}
