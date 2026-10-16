//! The discipline as a pure state machine: chrony's `ntp_core.c` (client
//! polling, packet tests, poll adaptation, iburst), `sources.c` (selection,
//! falsetickers, combining), `reference.c` (frequency and offset updates,
//! makestep, leap status) and the pool handling of `ntp_sources.c`.
//!
//! No sockets, threads, timers or DNS: the caller passes the host time into
//! every call, performs the [`Action`]s it returns (send a packet, resolve a
//! name) and calls [`Engine::poll`] again at [`Engine::next_wake`]. That
//! keeps it deterministic and lets the tests drive it through synthetic
//! networks.

use super::secs;
use core::net::{IpAddr, SocketAddr};

use super::config::{ClockConfig, SourceKind};
use super::leap::{self, DynamicLeap};
use super::log::{ClockLog, LogKind};
use super::mapping::MappingSnapshot;
use super::ntp::{
    KissCode, NtpPacket, NtpTimestamp, HEADER_LEN, LEAP_UNSYNC, MAX_STRATUM, MODE_SERVER,
};
use super::softclock::SoftwareClock;
use super::sourcestats::{Sample, SelectionData, SourceStats};

pub type SourceId = u32;

const NS: i64 = 1_000_000_000;
const SOURCE_REACH_BITS: u8 = 8;
const IBURST_GOOD_SAMPLES: i32 = 4;
const IBURST_TOTAL_SAMPLES: i32 = SOURCE_REACH_BITS as i32;
const MAX_BURST_INTERVAL: f64 = 2.0;
const MAX_BURST_POLL_RATIO: f64 = 0.25;
const INITIAL_DELAY: f64 = 0.2;
/// Minimum spacing between any two requests (chrony's sampling separation
/// at these poll intervals).
const SEPARATION: f64 = 0.2;
const SAMPLING_RANDOMNESS: f64 = 0.02;
/// Replies later than this after the request are ignored (chrony closes the
/// client socket after maxdelay + 4 s).
const MAX_SERVER_INTERVAL: f64 = 4.0;
const NTP_MAX_DISPERSION: f64 = 16.0;
const SCORE_LIMIT: f64 = 10.0;
const DISTANT_PENALTY: i32 = 32;
const BAD_HANDLE_THRESHOLD: i32 = 4;
const MAX_UNREACHABLE_RUN: u32 = 100_000;
/// Replacement of bad pool sources at most this often on average (chrony:
/// random up to 7 * 2^9 s).
const REPLACEMENT_INTERVAL: f64 = 7.0 * 512.0;
/// Re-resolution backoff after a failed or short DNS answer.
const RESOLVE_MIN: f64 = 8.0;
const RESOLVE_MAX: f64 = 512.0;
/// A replaced address is not picked again for this long.
const RECENTLY_REPLACED: f64 = 3600.0;
/// Without a clock update for this long the clock is no longer reported as
/// synchronized (holdover limit, 8 polls at maxpoll 8).
const HOLDOVER_LIMIT: f64 = 2048.0;

fn to_ns(s: f64) -> i64 {
    (s * 1e9).round() as i64
}

/// Randomness for request cookies and timer jitter. The platform supplies a
/// cryptographically strong one; tests use a seeded one.
pub trait RandomSource: Send {
    fn next_u64(&mut self) -> u64;
}

/// Deterministic generator for tests and simulations.
#[derive(Debug, Clone)]
pub struct SplitMix64(pub u64);

impl RandomSource for SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// What the caller has to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Send `packet` to `addr` from a fresh socket. Read the host clock right
    /// before sending and report it with [`Engine::on_transmitted`]; deliver
    /// the reply with [`Engine::on_packet`].
    Send {
        source: SourceId,
        addr: SocketAddr,
        packet: [u8; HEADER_LEN],
    },
    /// Resolve `name` (IPv4 and IPv6) and report with [`Engine::on_resolved`].
    Resolve { config_index: usize, name: String },
}

/// The network path the platform reports (e.g. NWPathMonitor).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PathInfo {
    pub satisfied: bool,
    pub ipv4: bool,
    pub ipv6: bool,
    /// Identity of the path (interface names/types); a change of it means a
    /// different delay context.
    pub key: String,
}

/// chrony's source states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceStatus {
    Ok,
    /// Not polled: refused by KoD DENY/RSTR, or the path lacks its address family.
    Offline,
    BadStats,
    Unsynchronised,
    BadStratum,
    BadDistance,
    Jittery,
    WaitsStats,
    Stale,
    Falseticker,
    WaitsSources,
    WaitsUpdate,
    Distant,
    Unselected,
    Selected,
}

impl SourceStatus {
    /// The `chronyc sources` state column.
    pub fn chronyc_char(self) -> char {
        match self {
            SourceStatus::Selected => '*',
            SourceStatus::Unselected => '+',
            SourceStatus::WaitsSources | SourceStatus::WaitsUpdate | SourceStatus::Distant => '-',
            SourceStatus::Falseticker => 'x',
            SourceStatus::Jittery => '~',
            _ => '?',
        }
    }

    fn selection_char(self) -> char {
        match self {
            SourceStatus::Ok => 'O',
            SourceStatus::Offline => 'N',
            SourceStatus::BadStats => 'M',
            SourceStatus::Unsynchronised => 's',
            SourceStatus::BadStratum => 'P',
            SourceStatus::BadDistance => 'D',
            SourceStatus::Jittery => '~',
            SourceStatus::WaitsStats => 'w',
            SourceStatus::Stale => 'S',
            SourceStatus::Falseticker => 'x',
            SourceStatus::WaitsSources => 'W',
            SourceStatus::WaitsUpdate => 'u',
            SourceStatus::Distant => 'D',
            SourceStatus::Unselected => '+',
            SourceStatus::Selected => '*',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeapStatus {
    Normal,
    Insert,
    Delete,
    Unsynchronised,
}

impl LeapStatus {
    pub fn name(self) -> &'static str {
        match self {
            LeapStatus::Normal => "normal",
            LeapStatus::Insert => "insert",
            LeapStatus::Delete => "delete",
            LeapStatus::Unsynchronised => "unsynchronised",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Burst {
    good_to_go: i32,
    total_to_go: i32,
}

#[derive(Debug, Clone, Copy)]
struct Outstanding {
    cookie: u64,
    t1: Option<i64>,
    sent_at: i64,
}

/// Last measurement, for reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LastMeasurement {
    pub host_ns: i64,
    /// Reference minus local, s (NTP sign).
    pub offset: f64,
    pub delay: f64,
    /// Root distance of the sample, s.
    pub error: f64,
}

#[derive(Debug, Clone)]
struct Source {
    id: SourceId,
    cfg_index: usize,
    name: String,
    addr: SocketAddr,
    stats: SourceStats,
    reach: u8,
    reach_size: u8,
    unreachable_run: u32,
    stratum: u8,
    leap: u8,
    ref_id: u32,
    local_poll: i8,
    poll_score: f64,
    min_poll: i8,
    max_poll: i8,
    burst: Option<Burst>,
    next_tx: i64,
    last_tx: i64,
    outstanding: Option<Outstanding>,
    tx_count: u32,
    remote_rx: NtpTimestamp,
    remote_tx: NtpTimestamp,
    status: SourceStatus,
    sel: Option<SelectionData>,
    sel_score: f64,
    distant: i32,
    bad: i32,
    updates: u32,
    leap_vote: bool,
    offline: bool,
    replace: bool,
    last: Option<LastMeasurement>,
    total_tx: u64,
    total_rx: u64,
    total_good: u64,
}

#[derive(Debug, Clone, Default)]
struct PoolState {
    resolving: bool,
    next_resolve: Option<i64>,
    backoff: f64,
    addresses: Vec<IpAddr>,
}

/// Counters for diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    pub sent: u64,
    pub received: u64,
    /// Replies with a wrong origin cookie (spoofed, stale or duplicate).
    pub bogus: u64,
    pub kod: u64,
    pub path_bursts: u64,
    pub replacements: u64,
    pub steps: u64,
}

/// chrony's `reference.c` state.
#[derive(Debug, Clone)]
struct Reference {
    synchronised: bool,
    /// Last selection found too few agreeing sources or no majority.
    disagreement: bool,
    stratum: u8,
    ref_source: Option<SourceId>,
    ref_addr: Option<SocketAddr>,
    ref_name: Option<String>,
    ref_time: i64,
    skew: f64,
    residual_freq: f64,
    root_delay: f64,
    root_dispersion: f64,
    offset_sd: f64,
    frequency_sd: f64,
    last_update: Option<i64>,
    last_update_interval: f64,
    last_offset: f64,
    avg2_offset: f64,
    avg2_moving: bool,
    make_step_limit: u32,
    leap: LeapStatus,
    dyn_leap: DynamicLeap,
    combined: usize,
    /// A step that was refused while live, done when the stream stops.
    pending_step: bool,
}

pub struct Engine {
    cfg: ClockConfig,
    rng: Box<dyn RandomSource>,
    log: ClockLog,
    clock: SoftwareClock,
    /// `R(h) = h + raw_base`: the raw timescale the samples are taken on.
    raw_base: i64,
    sources: Vec<Source>,
    next_id: SourceId,
    pools: Vec<PoolState>,
    selected: Option<SourceId>,
    last_updated: Option<SourceId>,
    reference: Reference,
    next_replacement: i64,
    recently_replaced: Vec<(IpAddr, i64)>,
    denied: Vec<IpAddr>,
    path: Option<PathInfo>,
    last_path_burst: Option<i64>,
    pending_path_burst: bool,
    last_tx_any: i64,
    counters: Counters,
    reported_no_majority: bool,
}

impl Engine {
    /// `wall_utc_ns`: the platform wall clock now, used until the first
    /// update (and as the raw timescale's origin).
    pub fn new(cfg: ClockConfig, now: i64, wall_utc_ns: i64, rng: Box<dyn RandomSource>) -> Self {
        let tai = leap::utc_to_tai_ns(wall_utc_ns, DynamicLeap::default());
        let clock = SoftwareClock::new(now, tai, cfg.max_slew_live, cfg.max_slew_idle);
        let pools = (0..cfg.sources.len())
            .map(|_| PoolState {
                next_resolve: Some(now),
                backoff: RESOLVE_MIN,
                ..Default::default()
            })
            .collect();
        let make_step_limit = cfg.make_step_limit;
        let drift = cfg.drift;
        let log = ClockLog::new(cfg.log_capacity);
        let mut e = Engine {
            cfg,
            rng,
            log,
            clock,
            raw_base: tai - now,
            sources: Vec::new(),
            next_id: 1,
            pools,
            selected: None,
            last_updated: None,
            reference: Reference {
                synchronised: false,
                disagreement: false,
                stratum: 0,
                ref_source: None,
                ref_addr: None,
                ref_name: None,
                ref_time: now,
                // chrony starts at 1.0 ("rather bad") without a driftfile. A
                // host oscillator is within the unknown-frequency bound of
                // 2000 ppm, so start there: the error bounds are honest and
                // the clock reports synchronised minutes earlier.
                skew: drift.map_or(super::sourcestats::WORST_CASE_FREQ_BOUND, |d| {
                    (d.1 * 1e-6).max(1e-12)
                }),
                residual_freq: 0.0,
                root_delay: 0.0,
                root_dispersion: 0.0,
                offset_sd: 0.0,
                frequency_sd: 0.0,
                last_update: None,
                last_update_interval: 0.0,
                last_offset: 0.0,
                avg2_offset: 0.0,
                avg2_moving: false,
                make_step_limit,
                leap: LeapStatus::Unsynchronised,
                dyn_leap: DynamicLeap::default(),
                combined: 0,
                pending_step: false,
            },
            next_replacement: now,
            recently_replaced: Vec::new(),
            denied: Vec::new(),
            path: None,
            last_path_burst: None,
            pending_path_burst: false,
            last_tx_any: i64::MIN / 2,
            counters: Counters::default(),
            reported_no_majority: false,
        };
        if let Some((freq_ppm, skew_ppm)) = drift {
            e.clock.set_base_freq(now, freq_ppm * 1e-6);
            e.event(
                now,
                format!("Frequency {freq_ppm:.3} ppm, skew {skew_ppm:.3} ppm from the drift file"),
            );
        }
        e.event(now, "Discipline started".into());
        e
    }

    /// (frequency ppm, skew ppm) to save like chrony's driftfile, once the
    /// clock has been updated.
    pub fn drift(&self) -> Option<(f64, f64)> {
        self.reference
            .last_update
            .map(|_| (self.clock.base_freq() * 1e6, self.reference.skew * 1e6))
    }

    pub fn config(&self) -> &ClockConfig {
        &self.cfg
    }

    pub fn log(&self) -> &ClockLog {
        &self.log
    }

    pub fn counters(&self) -> Counters {
        self.counters
    }

    /// The mapping to publish to hot-path readers.
    pub fn snapshot(&self, now: i64) -> MappingSnapshot {
        let mut s = *self.clock.snapshot();
        s.synced = self.is_synchronised(now);
        s
    }

    pub fn is_live(&self) -> bool {
        self.clock.is_live()
    }

    /// Synchronized: updated by at least `minsources` agreeing sources, no
    /// disagreement since, and not in holdover for longer than the limit.
    pub fn is_synchronised(&self, now: i64) -> bool {
        let r = &self.reference;
        r.synchronised
            && !r.disagreement
            && r.last_update
                .is_some_and(|t| secs(now - t) < HOLDOVER_LIMIT)
            && self.clock.snapshot().max_error_ns(now) < super::mapping::MAX_ERROR_CAP_NS
    }

    // ------------------------------------------------------------------
    // Timers
    // ------------------------------------------------------------------

    /// Runs what is due at `now` and returns the actions for the caller.
    pub fn poll(&mut self, now: i64) -> Vec<Action> {
        let mut actions = Vec::new();

        for i in 0..self.pools.len() {
            let p = &mut self.pools[i];
            if !p.resolving && p.next_resolve.is_some_and(|t| t <= now) {
                p.resolving = true;
                p.next_resolve = None;
                actions.push(Action::Resolve {
                    config_index: i,
                    name: self.cfg.sources[i].name.clone(),
                });
            }
        }

        // Late replies are no longer accepted.
        for s in self.sources.iter_mut() {
            if let Some(o) = s.outstanding {
                if secs(now - o.sent_at) > self.cfg.max_delay + MAX_SERVER_INTERVAL {
                    s.outstanding = None;
                }
            }
        }

        if self.pending_path_burst && self.path_burst_allowed(now) {
            self.start_path_burst(now);
        }

        // Transmissions, spaced by SEPARATION, oldest due first.
        loop {
            let due = self
                .sources
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.offline && s.next_tx <= now)
                .min_by_key(|(_, s)| s.next_tx)
                .map(|(i, _)| i);
            let Some(i) = due else { break };
            let earliest = self.last_tx_any + to_ns(SEPARATION);
            if now < earliest {
                // Push every due source behind the separation.
                let mut t = earliest;
                let mut due: Vec<usize> = (0..self.sources.len())
                    .filter(|&j| !self.sources[j].offline && self.sources[j].next_tx <= now)
                    .collect();
                due.sort_by_key(|&j| self.sources[j].next_tx);
                for j in due {
                    self.sources[j].next_tx = t;
                    t += to_ns(SEPARATION);
                }
                break;
            }
            if let Some(a) = self.transmit(now, i) {
                actions.push(a);
            }
        }

        // A slew that ran its planned course with a remainder continues.
        if now >= self.clock.slew_end() && self.clock.remaining(now).abs() > 1e-9 {
            self.clock.reslew(now);
        }

        actions
    }

    /// The next host time `poll` has something to do.
    pub fn next_wake(&self, now: i64) -> i64 {
        let mut t = now + 3600 * NS;
        for p in &self.pools {
            if let Some(r) = p.next_resolve {
                t = t.min(r);
            }
        }
        for s in &self.sources {
            if !s.offline {
                t = t.min(s.next_tx);
            }
            if let Some(o) = s.outstanding {
                t = t.min(o.sent_at + to_ns(self.cfg.max_delay + MAX_SERVER_INTERVAL) + 1);
            }
        }
        if self.clock.remaining(now).abs() > 1e-9 {
            t = t.min(self.clock.slew_end());
        }
        if self.pending_path_burst {
            if let Some(last) = self.last_path_burst {
                t = t.min(last + to_ns(self.cfg.path_burst_min_interval));
            }
        }
        t.max(now)
    }

    fn random_unit(&mut self) -> f64 {
        (self.rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn poll_interval(&self, s: &Source) -> f64 {
        2f64.powi(s.local_poll as i32)
    }

    /// chrony `get_transmit_delay` for client mode, plus its 2% randomness.
    fn transmit_delay(&mut self, i: usize) -> f64 {
        let interval = self.poll_interval(&self.sources[i]);
        let delay = if self.sources[i].burst.is_some() {
            MAX_BURST_INTERVAL.min(MAX_BURST_POLL_RATIO * interval)
        } else {
            interval
        };
        delay * (1.0 + SAMPLING_RANDOMNESS * self.random_unit())
    }

    /// chrony `transmit_timeout`.
    fn transmit(&mut self, now: i64, i: usize) -> Option<Action> {
        // With an online burst, switch back to online before the last packet.
        if let Some(b) = self.sources[i].burst {
            if b.total_to_go <= 1 {
                self.sources[i].burst = None;
            }
        }
        self.sources[i].tx_count += 1;
        if self.sources[i].tx_count >= 2 {
            // A response is missing.
            let is_selected = self.selected == Some(self.sources[i].id);
            self.adjust_poll(i, if is_selected { 0.1 } else { 0.25 });
            self.update_reachability(now, i, false);
        }
        if let Some(b) = self.sources[i].burst.as_mut() {
            b.total_to_go -= 1;
        }
        let mut cookie = self.rng.next_u64();
        if cookie == 0 {
            cookie = 1;
        }
        let synced = self.is_synchronised(now);
        let s = &mut self.sources[i];
        s.outstanding = Some(Outstanding {
            cookie,
            t1: None,
            sent_at: now,
        });
        s.last_tx = now;
        s.total_tx += 1;
        let packet = NtpPacket::client_request(cookie, s.local_poll, synced);
        let (id, addr) = (s.id, s.addr);
        self.last_tx_any = now;
        self.counters.sent += 1;
        let delay = self.transmit_delay(i);
        self.sources[i].next_tx = now + to_ns(delay);
        Some(Action::Send {
            source: id,
            addr,
            packet,
        })
    }

    /// The host time right before the request left (t1).
    pub fn on_transmitted(&mut self, source: SourceId, packet: &[u8; HEADER_LEN], t1: i64) {
        let cookie = u64::from_be_bytes(packet[40..48].try_into().unwrap());
        if let Some(s) = self.sources.iter_mut().find(|s| s.id == source) {
            if let Some(o) = s.outstanding.as_mut() {
                if o.cookie == cookie {
                    o.t1 = Some(t1);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Replies
    // ------------------------------------------------------------------

    /// A datagram from `source`'s socket, received at host time `t4`.
    pub fn on_packet(&mut self, t4: i64, source: SourceId, data: &[u8]) {
        let Some(i) = self.sources.iter().position(|s| s.id == source) else {
            return;
        };
        let Ok(pkt) = NtpPacket::decode(data) else {
            self.counters.bogus += 1;
            return;
        };
        if pkt.mode != MODE_SERVER {
            self.counters.bogus += 1;
            return;
        }
        // Test 2: the reply must echo the cookie of the outstanding request.
        let Some(out) = self.sources[i].outstanding else {
            self.counters.bogus += 1;
            return;
        };
        if pkt.origin_ts.0 != out.cookie {
            self.counters.bogus += 1;
            return;
        }
        let Some(t1) = out.t1 else {
            self.counters.bogus += 1;
            return;
        };
        let s = &self.sources[i];
        let test1 = pkt.receive_ts != s.remote_rx || pkt.transmit_ts != s.remote_tx;
        let test3 =
            !pkt.origin_ts.is_zero() && !pkt.receive_ts.is_zero() && !pkt.transmit_ts.is_zero();
        if !(test1 && test3) {
            self.counters.bogus += 1;
            return;
        }
        // At most one response per request.
        self.counters.received += 1;
        let s = &mut self.sources[i];
        s.outstanding = None;
        s.remote_rx = pkt.receive_ts;
        s.remote_tx = pkt.transmit_ts;
        s.tx_count = 0;
        s.total_rx += 1;

        let pkt_root_delay = pkt.root_delay_secs();
        let pkt_root_disp = pkt.root_dispersion_secs();
        let test6 = pkt.leap != LEAP_UNSYNC && pkt.stratum < MAX_STRATUM && pkt.stratum != 0;
        let test7 = pkt_root_delay / 2.0 + pkt_root_disp < NTP_MAX_DISPERSION;
        let synced_packet = test6 && test7;

        self.update_reachability(t4, i, synced_packet);
        if synced_packet {
            let s = &mut self.sources[i];
            s.stratum = pkt.stratum;
            s.leap = pkt.leap;
            s.ref_id = pkt.reference_id;
        }

        let mut kod_rate = false;
        if let Some(code) = pkt.kiss_code() {
            self.counters.kod += 1;
            match code {
                KissCode::Rate => kod_rate = true,
                KissCode::Deny | KissCode::Rstr => {
                    let addr = self.sources[i].addr;
                    self.event(
                        t4,
                        format!("Received KoD {:?} from {}, dropping it", code, addr.ip()),
                    );
                    self.denied.push(addr.ip());
                    self.sources[i].offline = true;
                    self.sources[i].status = SourceStatus::Offline;
                    self.request_replacement(t4, i, true);
                    self.select_source(t4, None);
                    return;
                }
                KissCode::Other(_) => {}
            }
        }

        let mut good = false;
        let mut sample = None;
        let (mut ta, mut tc) = (false, false);
        if synced_packet {
            let utc_now = self.utc_ns(t4);
            let dyn_leap = self.reference.dyn_leap;
            let t2_utc = pkt.receive_ts.to_unix_ns(utc_now);
            let t3_utc = pkt.transmit_ts.to_unix_ns(utc_now);
            let leap_close = leap::is_leap_close(t2_utc.div_euclid(NS), dyn_leap);
            let tai_utc = leap::tai_utc_at_utc(t2_utc.div_euclid(NS), dyn_leap) as i64 * NS;
            let (t2, t3) = (t2_utc + tai_utc, t3_utc + tai_utc);
            let l1 = t1 + self.raw_base;
            let l4 = t4 + self.raw_base;
            let remote_interval = t3 - t2;
            let local_interval = t4 - t1;
            let precision = self.cfg.precision + pkt.precision_secs();
            let mut delay = secs((local_interval - remote_interval).abs());
            if delay < precision {
                delay = precision;
            }
            let offset_ns = ((t2 as i128 - l1 as i128 + t3 as i128 - l4 as i128) / 2) as i64;
            let offset = secs(offset_ns);
            let sample_time = t1 + local_interval / 2;
            let (lo, hi) = self.sources[i].stats.frequency_range();
            let skew = (hi - lo) / 2.0;
            let peer_dispersion = precision + skew * secs(local_interval).abs();
            let response_time = secs(remote_interval).abs();

            ta = delay - peer_dispersion <= self.cfg.max_delay
                && response_time <= MAX_SERVER_INTERVAL
                && offset.abs() < 2f64.powi(31)
                && local_interval >= 0;
            tc = self.check_delay_dev_ratio(i, sample_time, offset, delay);
            good = ta && tc && !leap_close;
            sample = Some(Sample {
                time: sample_time,
                offset: -offset,
                orig_offset: -offset,
                peer_delay: delay,
                peer_dispersion,
                root_delay: pkt_root_delay + delay,
                root_dispersion: pkt_root_disp + peer_dispersion,
            });
        }

        // Measurements log (synced packets, like chrony without rawmeasurements).
        if let Some(smp) = sample {
            let s = &self.sources[i];
            let line = format!(
                "{} {:<15} {} {:2} {}{}{} {}{}{} {}{}{}{}  {:2} {:2} {:4.2} {:10.3e} {:10.3e} {:10.3e} {:10.3e} {:10.3e} {:08X}",
                self.date(t4),
                s.addr.ip(),
                ['N', '+', '-', '?'][pkt.leap as usize & 3],
                pkt.stratum,
                u8::from(test1),
                1,
                u8::from(test3),
                1,
                u8::from(test6),
                u8::from(test7),
                u8::from(ta),
                1,
                u8::from(tc),
                1,
                s.local_poll,
                pkt.poll,
                s.poll_score,
                -smp.offset,
                smp.peer_delay,
                smp.peer_dispersion,
                pkt_root_delay,
                pkt_root_disp,
                pkt.reference_id
            );
            self.log.push(LogKind::Measurements, line);
            self.sources[i].last = Some(LastMeasurement {
                host_ns: smp.time,
                offset: -smp.offset,
                delay: smp.peer_delay,
                error: 0.5 * smp.root_delay + smp.root_dispersion,
            });
        }

        if good {
            self.process_sample(t4, i, sample.unwrap());
            self.sources[i].total_good += 1;
            if let Some(b) = self.sources[i].burst.as_mut() {
                b.good_to_go -= 1;
                if b.good_to_go <= 0 {
                    self.sources[i].burst = None;
                }
            }
        } else {
            // Slowly increase the polling interval without good responses.
            self.adjust_poll(i, 0.02);
        }

        // Next request: one interval after the last one.
        let delay = self.transmit_delay(i);
        let s = &mut self.sources[i];
        let mut next = s.last_tx + to_ns(delay);
        if kod_rate {
            next += to_ns(4.0 * 2f64.powi(s.local_poll as i32));
            s.burst = None;
            let addr = s.addr.ip();
            self.event(t4, format!("Received KoD RATE from {addr}"));
        }
        self.sources[i].next_tx = next.max(t4);
    }

    /// chrony test C (`check_delay_dev_ratio`).
    fn check_delay_dev_ratio(&self, i: usize, sample_time: i64, offset: f64, delay: f64) -> bool {
        let Some((ago, predicted, min_delay, skew, std_dev)) =
            self.sources[i].stats.delay_test_data(sample_time)
        else {
            return true;
        };
        let max_delta =
            std_dev * self.cfg.max_delay_dev_ratio + ago * (skew + self.cfg.max_clock_error);
        let delta = (delay - min_delay) / 2.0;
        if delta <= max_delta {
            return true;
        }
        // Keep it when the offset moved more than the extra delay explains.
        let error_in_estimate = offset + predicted;
        error_in_estimate.abs() - delta > max_delta
    }

    /// chrony `process_sample`.
    fn process_sample(&mut self, now: i64, i: usize, mut sample: Sample) {
        // Before the first update, move the raw timescale onto the sources
        // when the platform clock was far off, so offsets stay small.
        if self.reference.last_update.is_none() && sample.offset.abs() > 1.0 {
            let shift = to_ns(-sample.offset);
            self.raw_base += shift;
            let d = secs(shift);
            for s in self.sources.iter_mut() {
                s.stats.correct_offset(d);
            }
            sample.offset += d;
            sample.orig_offset += d;
            self.event(now, format!("Raw timescale rebased by {:.3} s", d));
        }
        let predicted = self.sources[i].stats.predict_offset(sample.time);
        let error_in_estimate = (sample.offset - predicted).abs();
        let peer_distance = sample.peer_dispersion + 0.5 * sample.peer_delay;

        let (old_freq, old_skew) = (
            self.sources[i].stats.frequency(),
            self.sources[i].stats.skew(),
        );
        self.sources[i].stats.accumulate(sample);
        self.sources[i].stats.do_new_regression();
        {
            let s = &self.sources[i];
            if s.stats.regression_ok() {
                let st = &s.stats;
                let td = st.tracking_data().unwrap();
                let line = format!(
                    "{} {:<15} {:10.3e} {:10.3e} {:10.3e} {:10.3e} {:10.3e} {:7.1e} {:3} {:3} {:3} {:5.2}",
                    self.date(now),
                    s.addr.ip(),
                    st.std_dev(),
                    td.offset,
                    td.offset_sd,
                    td.frequency,
                    td.skew,
                    (old_freq - td.frequency).abs() / old_skew,
                    st.n_samples() + st.last_best_start(),
                    st.last_best_start(),
                    st.n_runs(),
                    st.asymmetry()
                );
                self.log.push(LogKind::Statistics, line);
            }
        }
        self.select_source(now, Some(self.sources[i].id));
        let adj = self.poll_adj(i, error_in_estimate, peer_distance);
        self.adjust_poll(i, adj);
    }

    /// chrony `get_poll_adj`.
    fn poll_adj(&self, i: usize, error_in_estimate: f64, peer_distance: f64) -> f64 {
        if error_in_estimate > peer_distance {
            // Not tracking the source well: poll more often.
            -(error_in_estimate / peer_distance).ln() / 2f64.ln()
        } else {
            let samples = self.sources[i].stats.n_samples() as f64;
            let target = self.cfg.poll_target;
            let mut adj = (samples / target - 1.0) / target;
            if samples < target {
                adj *= 2.0;
            }
            adj
        }
    }

    /// chrony `adjust_poll`: the poll never leaves [minpoll, maxpoll].
    fn adjust_poll(&mut self, i: usize, adj: f64) {
        let s = &mut self.sources[i];
        s.poll_score += adj;
        if s.poll_score >= 1.0 {
            s.local_poll = s.local_poll.saturating_add(s.poll_score as i8);
            s.poll_score -= (s.poll_score as i64) as f64;
        }
        if s.poll_score < 0.0 {
            let d = (s.poll_score - 1.0) as i64;
            s.local_poll = (s.local_poll as i64 + d).clamp(-128, 127) as i8;
            s.poll_score -= d as f64;
        }
        if s.local_poll < s.min_poll {
            s.local_poll = s.min_poll;
            s.poll_score = 0.0;
        } else if s.local_poll > s.max_poll {
            s.local_poll = s.max_poll;
            s.poll_score = 1.0;
        }
    }

    /// chrony `SRC_UpdateReachability`.
    fn update_reachability(&mut self, now: i64, i: usize, reachable: bool) {
        let s = &mut self.sources[i];
        s.reach = (s.reach << 1) | u8::from(reachable);
        if s.reach_size < SOURCE_REACH_BITS {
            s.reach_size += 1;
        }
        if s.reach == 0 {
            s.unreachable_run = s.unreachable_run.saturating_add(1);
        } else {
            s.unreachable_run = 0;
        }
        if s.reach == 0 && s.reach_size == SOURCE_REACH_BITS {
            self.request_replacement(now, i, false);
        }
        if self.sources[i].reach == 0 {
            self.select_source(now, None);
        }
    }

    // ------------------------------------------------------------------
    // Pools
    // ------------------------------------------------------------------

    /// DNS answer for `config_index` (`Err` or empty on failure).
    pub fn on_resolved(
        &mut self,
        now: i64,
        config_index: usize,
        result: Result<Vec<IpAddr>, String>,
    ) {
        let Some(pool) = self.pools.get_mut(config_index) else {
            return;
        };
        pool.resolving = false;
        let addrs = match result {
            Ok(a) if !a.is_empty() => a,
            other => {
                let why = other.err().unwrap_or_else(|| "no addresses".into());
                let backoff = pool.backoff;
                pool.next_resolve = Some(now + to_ns(backoff));
                pool.backoff = (backoff * 2.0).min(RESOLVE_MAX);
                let name = self.cfg.sources[config_index].name.clone();
                self.event(
                    now,
                    format!("Could not resolve {name}: {why}, retry in {backoff:.0} s"),
                );
                return;
            }
        };
        pool.addresses = addrs.clone();
        pool.backoff = RESOLVE_MIN;
        let cfg = self.cfg.sources[config_index].clone();
        let port = cfg.port;
        self.recently_replaced.retain(|(_, until)| *until > now);

        // Replace sources that asked for it, then fill up to maxsources.
        let mut candidates: Vec<IpAddr> = addrs
            .into_iter()
            .filter(|a| self.family_ok(a))
            .filter(|a| !self.denied.contains(a))
            .filter(|a| !self.recently_replaced.iter().any(|(r, _)| r == a))
            .filter(|a| !self.sources.iter().any(|s| s.addr.ip() == *a && !s.replace))
            .collect();
        candidates.dedup();

        let mut replaced = 0;
        for i in 0..self.sources.len() {
            if self.sources[i].cfg_index != config_index || !self.sources[i].replace {
                continue;
            }
            let Some(new_ip) = candidates.first().copied() else {
                break;
            };
            candidates.remove(0);
            let old = self.sources[i].addr;
            self.recently_replaced
                .push((old.ip(), now + to_ns(RECENTLY_REPLACED)));
            let id = self.alloc_id();
            let fresh = self.new_source(now, id, config_index, SocketAddr::new(new_ip, port));
            if self.selected == Some(self.sources[i].id) {
                self.selected = None;
            }
            self.sources[i] = fresh;
            replaced += 1;
            self.counters.replacements += 1;
            self.event(
                now,
                format!(
                    "Source {} replaced with {} ({})",
                    old.ip(),
                    new_ip,
                    cfg.name
                ),
            );
        }
        for s in self.sources.iter_mut() {
            if s.cfg_index == config_index && s.replace && replaced == 0 {
                // Nothing to replace it with yet; keep using it.
                s.replace = false;
            }
        }
        let active = self
            .sources
            .iter()
            .filter(|s| s.cfg_index == config_index && !s.offline)
            .count();
        let wanted = cfg.max_sources.saturating_sub(active);
        for ip in candidates.into_iter().take(wanted) {
            let id = self.alloc_id();
            let src = self.new_source(now, id, config_index, SocketAddr::new(ip, port));
            self.event(now, format!("Added source {} ({})", ip, cfg.name));
            self.sources.push(src);
        }
        let active = self
            .sources
            .iter()
            .filter(|s| s.cfg_index == config_index && !s.offline)
            .count();
        if active < cfg.max_sources && cfg.kind == SourceKind::Pool {
            // Too few addresses this time; ask again later.
            let p = &mut self.pools[config_index];
            p.next_resolve = Some(now + to_ns(p.backoff));
            p.backoff = (p.backoff * 2.0).min(RESOLVE_MAX);
        }
    }

    fn family_ok(&self, a: &IpAddr) -> bool {
        match &self.path {
            Some(p) if p.satisfied => match a {
                IpAddr::V4(_) => p.ipv4,
                IpAddr::V6(_) => p.ipv6,
            },
            _ => true,
        }
    }

    fn alloc_id(&mut self) -> SourceId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn new_source(&mut self, now: i64, id: SourceId, cfg_index: usize, addr: SocketAddr) -> Source {
        let c = &self.cfg.sources[cfg_index];
        // Spread first requests: INITIAL_DELAY plus a little per source.
        let first = now + to_ns(INITIAL_DELAY);
        Source {
            id,
            cfg_index,
            name: c.name.clone(),
            addr,
            stats: SourceStats::new(self.cfg.precision),
            reach: 0,
            reach_size: 0,
            unreachable_run: 0,
            stratum: 0,
            leap: LEAP_UNSYNC,
            ref_id: 0,
            local_poll: c.min_poll,
            poll_score: 0.0,
            min_poll: c.min_poll,
            max_poll: c.max_poll,
            burst: c.iburst.then_some(Burst {
                good_to_go: IBURST_GOOD_SAMPLES,
                total_to_go: IBURST_TOTAL_SAMPLES,
            }),
            next_tx: first,
            last_tx: now,
            outstanding: None,
            tx_count: 0,
            remote_rx: NtpTimestamp(0),
            remote_tx: NtpTimestamp(0),
            status: SourceStatus::BadStats,
            sel: None,
            sel_score: 1.0,
            distant: 0,
            bad: 0,
            updates: 0,
            leap_vote: false,
            offline: !self.family_ok(&addr.ip()),
            replace: false,
            last: None,
            total_tx: 0,
            total_rx: 0,
            total_good: 0,
        }
    }

    /// chrony `NSR_HandleBadSource`: replace a pool source, rate-limited.
    fn request_replacement(&mut self, now: i64, i: usize, urgent: bool) {
        let cfg_index = self.sources[i].cfg_index;
        let is_literal = self.cfg.sources[cfg_index].name.parse::<IpAddr>().is_ok();
        if is_literal || self.sources[i].replace {
            return;
        }
        if !urgent && now < self.next_replacement {
            return;
        }
        if !urgent {
            self.next_replacement = now + to_ns(self.random_unit() * REPLACEMENT_INTERVAL);
        }
        self.sources[i].replace = true;
        let p = &mut self.pools[cfg_index];
        if !p.resolving {
            p.next_resolve = Some(now);
        }
    }

    // ------------------------------------------------------------------
    // Platform events
    // ------------------------------------------------------------------

    /// The network path changed (Wi-Fi <-> cellular, another interface).
    /// Samples taken on the old path describe another delay context and are
    /// dropped; a new iburst follows, at most once per path change and not
    /// more often than `path_burst_min_interval`.
    pub fn on_path_change(&mut self, now: i64, path: PathInfo) {
        let changed = self
            .path
            .as_ref()
            .map(|p| p.key != path.key || p.satisfied != path.satisfied)
            .unwrap_or(true);
        let first = self.path.is_none();
        self.path = Some(path.clone());
        // Address families the path cannot reach.
        for i in 0..self.sources.len() {
            let ok = self.family_ok(&self.sources[i].addr.ip());
            let denied = self.denied.contains(&self.sources[i].addr.ip());
            if !ok && !self.sources[i].offline {
                self.sources[i].offline = true;
                self.sources[i].status = SourceStatus::Offline;
                self.request_replacement(now, i, true);
            } else if ok && self.sources[i].offline && !denied {
                self.sources[i].offline = false;
                self.sources[i].next_tx = now + to_ns(INITIAL_DELAY);
            }
        }
        if !changed || first || !path.satisfied {
            if path.satisfied {
                // Retry failed resolutions on any usable path.
                for p in self.pools.iter_mut() {
                    if !p.resolving && p.addresses.is_empty() {
                        p.next_resolve = Some(now);
                    }
                }
            }
            return;
        }
        self.event(now, format!("Network path changed to {}", path.key));
        for s in self.sources.iter_mut() {
            s.stats.reset();
            s.sel = None;
            s.status = SourceStatus::BadStats;
            s.outstanding = None;
            s.tx_count = 0;
        }
        for p in self.pools.iter_mut() {
            if !p.resolving && p.addresses.is_empty() {
                p.next_resolve = Some(now);
            }
        }
        self.pending_path_burst = true;
        if self.path_burst_allowed(now) {
            self.start_path_burst(now);
        }
    }

    fn path_burst_allowed(&self, now: i64) -> bool {
        self.last_path_burst
            .is_none_or(|t| secs(now - t) >= self.cfg.path_burst_min_interval)
    }

    fn start_path_burst(&mut self, now: i64) {
        self.pending_path_burst = false;
        self.last_path_burst = Some(now);
        self.counters.path_bursts += 1;
        for s in self.sources.iter_mut() {
            if s.offline {
                continue;
            }
            if self.cfg.sources[s.cfg_index].iburst {
                s.burst = Some(Burst {
                    good_to_go: IBURST_GOOD_SAMPLES,
                    total_to_go: IBURST_TOTAL_SAMPLES,
                });
            }
            s.next_tx = now + to_ns(INITIAL_DELAY);
        }
    }

    /// The host clock stood still for `delta_ns` while the device slept
    /// (mach_absolute_time does not count sleep). The mapping is moved on by
    /// the sleep time; the samples are reset and the clock counts as
    /// unsynchronised until new samples confirm it (chrony's unknown step).
    pub fn on_host_paused(&mut self, now: i64, delta_ns: i64) {
        self.clock.host_paused(now, delta_ns);
        self.raw_base += delta_ns;
        for s in self.sources.iter_mut() {
            s.stats.reset();
            s.outstanding = None;
            s.tx_count = 0;
        }
        self.reference.synchronised = false;
        self.reference.last_update = None;
        self.event(
            now,
            format!(
                "Host clock paused for {:.3} s (sleep), samples reset",
                secs(delta_ns)
            ),
        );
        self.pending_path_burst = true;
        if self.path_burst_allowed(now) {
            self.start_path_burst(now);
        }
    }

    /// While live, the mapping is never stepped and slews at most
    /// `max_slew_live`. A step that was refused while live happens when the
    /// stream stops.
    pub fn set_live(&mut self, now: i64, live: bool) {
        self.clock.set_live(now, live);
        if !live && self.reference.pending_step {
            self.reference.pending_step = false;
            let remaining = self.clock.remaining(now);
            if remaining.abs() > self.cfg.make_step_threshold {
                self.clock.step(now, 0.0);
                self.counters.steps += 1;
                self.event(
                    now,
                    format!("Clock stepped by {:+.6} s (deferred while live)", remaining),
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // Selection (chrony `SRC_SelectSource`)
    // ------------------------------------------------------------------

    fn mark(&mut self, now: i64, i: usize, status: SourceStatus) {
        let s = &mut self.sources[i];
        let changed = s.status != status;
        s.status = status;
        if Some(s.id) == self.last_updated {
            if matches!(
                status,
                SourceStatus::Falseticker
                    | SourceStatus::BadDistance
                    | SourceStatus::BadStratum
                    | SourceStatus::Jittery
            ) {
                s.bad = s.bad.saturating_add(1);
            } else {
                s.bad = 0;
            }
            if s.bad >= BAD_HANDLE_THRESHOLD {
                self.request_replacement(now, i, false);
            }
        }
        let s = &self.sources[i];
        let line = format!(
            "{} {:<15} {} -{}--- {:4o} {:5.2} {:10.3e} {:10.3e} {:10.3e}",
            self.date(now),
            s.addr.ip(),
            status.selection_char(),
            if s.offline { 'N' } else { '-' },
            s.reach,
            s.sel_score,
            s.sel.map_or(0.0, |d| d.last_sample_ago),
            s.sel.map_or(0.0, |d| -d.hi_limit),
            s.sel.map_or(0.0, |d| -d.lo_limit),
        );
        self.log.push(LogKind::Selection, line);
        if changed && status == SourceStatus::Falseticker {
            let ip = self.sources[i].addr.ip();
            self.event(now, format!("Detected falseticker {ip}"));
        }
    }

    fn mark_ok(&mut self, now: i64, status: SourceStatus) {
        for i in 0..self.sources.len() {
            if self.sources[i].status == SourceStatus::Ok {
                self.mark(now, i, status);
            }
        }
    }

    fn unselect(&mut self, now: i64, why: Option<&str>) {
        let had = self.selected.take().is_some();
        if let Some(why) = why {
            if had {
                self.event(now, format!("Can't synchronise: {why}"));
            }
        }
    }

    fn select_source(&mut self, now: i64, updated: Option<SourceId>) {
        if let Some(id) = updated {
            if let Some(s) = self.sources.iter_mut().find(|s| s.id == id) {
                s.updates += 1;
            }
            self.last_updated = Some(id);
        }
        if self.sources.is_empty() {
            self.unselect(now, Some("no sources"));
            return;
        }
        let n = self.sources.len();
        let mut n_badstats = 0;
        let mut max_badstat_reach = 0u8;
        let mut max_badstat_reach_size = 0u8;
        let mut max_sel_reach = 0u8;
        let mut max_sel_reach_size = 0u8;
        let mut max_reach_sample_ago = 0.0f64;

        for i in 0..n {
            self.sources[i].leap_vote = false;
            if self.sources[i].offline {
                self.mark(now, i, SourceStatus::Offline);
                continue;
            }
            let Some(mut sd) = self.sources[i].stats.selection_data(now) else {
                n_badstats += 1;
                self.sources[i].sel = None;
                self.mark(now, i, SourceStatus::BadStats);
                max_badstat_reach = max_badstat_reach.max(self.sources[i].reach);
                max_badstat_reach_size = max_badstat_reach_size.max(self.sources[i].reach_size);
                continue;
            };
            if self.sources[i].leap == LEAP_UNSYNC {
                self.mark(now, i, SourceStatus::Unsynchronised);
                continue;
            }
            if self.sources[i].stratum == 0 || self.sources[i].stratum > self.cfg.max_stratum {
                self.mark(now, i, SourceStatus::BadStratum);
                continue;
            }
            // Extra dispersion for sources without new samples.
            if sd.first_sample_ago < 2.0 * sd.last_sample_ago {
                let extra =
                    self.cfg.max_clock_error * (2.0 * sd.last_sample_ago - sd.first_sample_ago);
                sd.root_distance += extra;
                sd.lo_limit -= extra;
                sd.hi_limit += extra;
            }
            self.sources[i].sel = Some(sd);
            if !(sd.root_distance <= self.cfg.max_distance && sd.lo_limit <= sd.hi_limit) {
                self.mark(now, i, SourceStatus::BadDistance);
                continue;
            }
            if sd.std_dev > self.cfg.max_jitter {
                self.mark(now, i, SourceStatus::Jittery);
                continue;
            }
            self.sources[i].status = SourceStatus::Ok;
            if self.sources[i].reach != 0 && max_reach_sample_ago < sd.first_sample_ago {
                max_reach_sample_ago = sd.first_sample_ago;
            }
            max_sel_reach = max_sel_reach.max(self.sources[i].reach);
            max_sel_reach_size = max_sel_reach_size.max(self.sources[i].reach_size);
        }

        let mut n_sel = 0;
        for i in 0..n {
            if self.sources[i].status != SourceStatus::Ok {
                continue;
            }
            let sd = self.sources[i].sel.unwrap();
            if self.sources[i].reach == 0
                && (sd.last_sample_ago > max_reach_sample_ago
                    || self.sources[i].unreachable_run > MAX_UNREACHABLE_RUN)
            {
                self.mark(now, i, SourceStatus::Stale);
                continue;
            }
            n_sel += 1;
        }

        // On start, wait until sources polled in step all have statistics.
        if n_badstats > 0
            && n_sel > 0
            && self.selected.is_none()
            && max_sel_reach_size < SOURCE_REACH_BITS
            && max_sel_reach >> 1 == max_badstat_reach
        {
            self.mark_ok(now, SourceStatus::WaitsStats);
            self.unselect(now, None);
            return;
        }

        // Marzullo-style intersection: the interval contained in the most
        // source intervals.
        let mut endpoints: Vec<(f64, i8)> = Vec::with_capacity(2 * n_sel);
        for s in &self.sources {
            if s.status == SourceStatus::Ok {
                let sd = s.sel.unwrap();
                endpoints.push((sd.lo_limit, -1));
                endpoints.push((sd.hi_limit, 1));
            }
        }
        if endpoints.is_empty() {
            self.unselect(now, Some("no selectable sources"));
            return;
        }
        endpoints.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let (mut depth, mut best_depth) = (0usize, 0usize);
        let (mut best_lo, mut best_hi) = (0.0, 0.0);
        for &(v, tag) in &endpoints {
            if tag < 0 {
                depth += 1;
                if depth > best_depth {
                    best_depth = depth;
                    best_lo = v;
                }
            } else {
                if depth == best_depth {
                    best_hi = v;
                }
                depth -= 1;
            }
        }

        if best_depth <= n_sel / 2 {
            if !self.reported_no_majority {
                self.reported_no_majority = true;
                self.event(now, format!("Can't synchronise: no majority (only {best_depth} of {n_sel} sources agree)"));
            }
            if self.selected.is_some() {
                self.selected = None;
            }
            self.set_unsynchronised(now);
            self.mark_ok(now, SourceStatus::Falseticker);
            return;
        }

        let mut sel_list: Vec<usize> = Vec::new();
        for i in 0..n {
            if self.sources[i].status != SourceStatus::Ok {
                continue;
            }
            let sd = self.sources[i].sel.unwrap();
            if (sd.lo_limit <= best_lo && sd.hi_limit >= best_hi)
                || (sd.lo_limit >= best_lo && sd.hi_limit <= best_hi)
            {
                sel_list.push(i);
            } else {
                self.mark(now, i, SourceStatus::Falseticker);
            }
        }

        if sel_list.len() < self.cfg.min_sources.max(1) {
            let why = if sel_list.is_empty() {
                "no selectable sources"
            } else {
                "not enough selectable sources"
            };
            self.unselect(now, Some(why));
            // Stricter than chrony: fewer than minsources agreeing sources
            // also means not synchronized (the clock keeps free running).
            if !self.reference.disagreement && self.reference.synchronised {
                self.event(
                    now,
                    format!(
                        "Only {} of minsources {} agree, clock in holdover",
                        sel_list.len(),
                        self.cfg.min_sources
                    ),
                );
            }
            self.reference.disagreement = true;
            self.mark_ok(now, SourceStatus::WaitsSources);
            return;
        }

        for &i in &sel_list {
            self.sources[i].leap_vote = true;
        }

        let min_stratum = sel_list
            .iter()
            .map(|&i| self.sources[i].stratum)
            .min()
            .unwrap();
        let selected_idx = self
            .selected
            .and_then(|id| self.sources.iter().position(|s| s.id == id));
        let sel_src_distance = selected_idx.map_or(0.0, |k| {
            self.sources[k].sel.map_or(0.0, |d| d.root_distance)
                + (self.sources[k].stratum - min_stratum.min(self.sources[k].stratum)) as f64
                    * self.cfg.stratum_weight
        });

        let mut max_score = 0.0;
        let mut max_score_index = None;
        for i in 0..n {
            if self.sources[i].status != SourceStatus::Ok {
                self.sources[i].sel_score = 1.0;
                self.sources[i].distant = DISTANT_PENALTY;
                continue;
            }
            let sd = self.sources[i].sel.unwrap();
            let distance = sd.root_distance
                + (self.sources[i].stratum - min_stratum) as f64 * self.cfg.stratum_weight
                + self.cfg.reselect_distance;
            if let Some(k) = selected_idx {
                if Some(self.sources[i].id) == updated || Some(self.sources[k].id) == updated {
                    let s = &mut self.sources[i];
                    s.sel_score *= sel_src_distance / distance;
                    if s.sel_score < 1.0 {
                        s.sel_score = 1.0;
                    }
                }
            } else {
                self.sources[i].sel_score = 1.0 / distance;
            }
            if max_score < self.sources[i].sel_score {
                max_score = self.sources[i].sel_score;
                max_score_index = Some(i);
            }
        }
        let max_score_index = max_score_index.unwrap();

        let current_ok = selected_idx.is_some_and(|k| self.sources[k].status == SourceStatus::Ok);
        let mut selected_idx = selected_idx;
        if !current_ok || (Some(max_score_index) != selected_idx && max_score > SCORE_LIMIT) {
            if self.sources[max_score_index].updates == 0 {
                self.unselect(now, None);
                self.mark_ok(now, SourceStatus::WaitsUpdate);
                return;
            }
            self.selected = Some(self.sources[max_score_index].id);
            selected_idx = Some(max_score_index);
            for s in self.sources.iter_mut() {
                s.sel_score = 1.0;
                s.distant = 0;
            }
            self.reported_no_majority = false;
            let s = &self.sources[max_score_index];
            let msg = format!("Selected source {} ({})", s.addr.ip(), s.name);
            self.event(now, msg);
        }
        let k = selected_idx.unwrap();
        self.mark(now, k, SourceStatus::Selected);

        if self.sources[k].updates == 0 {
            for &i in &sel_list {
                if self.sources[i].status == SourceStatus::Ok {
                    let st = if self.sources[i].distant != 0 {
                        SourceStatus::Distant
                    } else {
                        SourceStatus::Unselected
                    };
                    self.mark(now, i, st);
                }
            }
            return;
        }
        for s in self.sources.iter_mut() {
            s.updates = 0;
        }

        let leap = self.voted_leap();
        let Some(mut td) = self.sources[k].stats.tracking_data() else {
            return;
        };
        let combined = self.combine_sources(now, &sel_list, k, &mut td);
        let stratum = self.sources[k].stratum;
        self.set_reference(now, k, stratum, leap, combined, td);
    }

    /// chrony `combine_sources`.
    fn combine_sources(
        &mut self,
        now: i64,
        sel_list: &[usize],
        k: usize,
        td: &mut super::sourcestats::TrackingData,
    ) -> usize {
        if sel_list.len() == 1 {
            return 1;
        }
        let sel_src_distance =
            self.sources[k].sel.map_or(0.0, |d| d.root_distance) + self.cfg.reselect_distance;
        let (mut sum_ow, mut sum_o, mut sum2_osd) = (0.0, 0.0, 0.0);
        let (mut sum_fw, mut sum_f, mut inv_sum2_fsd, mut inv_sum2_skew) = (0.0, 0.0, 0.0, 0.0);
        let mut combined = 0;
        let (ref_time, offset0, frequency0, skew0) =
            (td.ref_time, td.offset, td.frequency, td.skew);
        for &i in sel_list {
            let Some(src) = self.sources[i].stats.tracking_data() else {
                continue;
            };
            let dist = self.sources[i].sel.map_or(f64::MAX, |d| d.root_distance);
            if i != k
                && (dist > self.cfg.combine_limit * sel_src_distance
                    || (frequency0 - src.frequency).abs()
                        > self.cfg.combine_limit * (skew0 + src.skew + self.cfg.max_clock_error))
            {
                self.sources[i].distant = if self.sources[i].reach_size >= SOURCE_REACH_BITS {
                    DISTANT_PENALTY
                } else {
                    1
                };
            } else if self.sources[i].distant > 0 {
                self.sources[i].distant -= 1;
            }
            if self.sources[i].distant > 0 {
                self.mark(now, i, SourceStatus::Distant);
                continue;
            }
            if self.sources[i].status == SourceStatus::Ok {
                self.mark(now, i, SourceStatus::Unselected);
            }
            let elapsed = secs(ref_time - src.ref_time);
            let src_offset = src.offset + elapsed * src.frequency;
            let src_offset_sd = src.offset_sd + elapsed * src.frequency_sd;
            let ow = 1.0 / dist;
            let fw = 1.0 / (src.frequency_sd * src.frequency_sd);
            sum_ow += ow;
            sum_o += ow * src_offset;
            sum2_osd += ow
                * (src_offset_sd * src_offset_sd + (src_offset - offset0) * (src_offset - offset0));
            sum_fw += fw;
            sum_f += fw * src.frequency;
            inv_sum2_fsd += fw;
            inv_sum2_skew += 1.0 / (src.skew * src.skew);
            combined += 1;
        }
        if combined == 0 {
            return 1;
        }
        td.offset = sum_o / sum_ow;
        td.offset_sd = (sum2_osd / sum_ow).sqrt();
        td.frequency = sum_f / sum_fw;
        td.frequency_sd = 1.0 / inv_sum2_fsd.sqrt();
        td.skew = 1.0 / inv_sum2_skew.sqrt();
        combined
    }

    fn voted_leap(&self) -> LeapStatus {
        let (mut votes, mut ins, mut del) = (0, 0, 0);
        for s in self.sources.iter().filter(|s| s.leap_vote) {
            votes += 1;
            match s.leap {
                1 => ins += 1,
                2 => del += 1,
                _ => {}
            }
        }
        if ins > votes / 2 {
            LeapStatus::Insert
        } else if del > votes / 2 {
            LeapStatus::Delete
        } else {
            LeapStatus::Normal
        }
    }

    // ------------------------------------------------------------------
    // Reference update (chrony `REF_SetReference`)
    // ------------------------------------------------------------------

    fn root_dispersion_at(&self, now: i64) -> f64 {
        let r = &self.reference;
        if r.last_update.is_none() {
            return 1.0;
        }
        r.root_dispersion
            + secs(now - r.ref_time).abs()
                * (r.skew + r.residual_freq.abs() + self.cfg.max_clock_error)
    }

    fn set_reference(
        &mut self,
        now: i64,
        k: usize,
        stratum: u8,
        voted_leap: LeapStatus,
        combined: usize,
        td: super::sourcestats::TrackingData,
    ) {
        let elapsed = secs(now - td.ref_time);
        let raw_offset_now = td.offset + elapsed * td.frequency;
        let target_now = now + self.raw_base - to_ns(raw_offset_now);
        let d_now = self.clock.tai_ns(now);
        let remaining = self.clock.remaining(now);
        // chrony's offset against the cooked clock (pending correction applied).
        let offset = secs(d_now - target_now) + remaining;
        let update_interval = self.reference.last_update.map_or(0.0, |t| secs(now - t));

        // New frequency and skew: weighted average of the expected (no
        // change) and the measured frequency.
        let measured_freq = self.clock.base_freq() + td.frequency;
        let expected_skew = self.reference.skew;
        let gain = if td.skew.abs() > self.cfg.max_update_skew {
            0.0
        } else {
            (3.0 * expected_skew * expected_skew
                / (3.0 * expected_skew * expected_skew + td.skew * td.skew))
                .clamp(0.0, 1.0)
        };
        let est_freq = gain * measured_freq;
        let residual = measured_freq - est_freq;
        let extra_skew = (est_freq * est_freq * (1.0 - gain) + residual * residual * gain).sqrt();
        let est_skew = expected_skew + gain * (td.skew - expected_skew) + extra_skew;

        let orig_root_distance = self.reference.root_delay / 2.0 + self.root_dispersion_at(now);
        let src = &self.sources[k];
        let r = &mut self.reference;
        let first = r.last_update.is_none();
        r.synchronised = true;
        r.disagreement = false;
        r.stratum = stratum.saturating_add(1);
        r.ref_source = Some(src.id);
        r.ref_addr = Some(src.addr);
        r.ref_name = Some(src.name.clone());
        r.ref_time = td.ref_time;
        r.skew = est_skew;
        r.residual_freq = residual;
        r.root_delay = td.root_delay;
        r.root_dispersion = td.root_dispersion;
        r.frequency_sd = td.frequency_sd;
        r.offset_sd = td.offset_sd;
        r.last_update = Some(now);
        r.last_update_interval = update_interval;
        r.last_offset = offset;
        r.combined = combined;

        // makestep 1.0 3: a step only in the first updates, only when the
        // clock is more than the threshold off, and never while live.
        let mut step = false;
        if r.make_step_limit > 0 {
            r.make_step_limit -= 1;
            step = (offset - remaining).abs() > self.cfg.make_step_threshold;
        }
        let correction_rate = self.cfg.corr_time_ratio * 0.5 * td.offset_sd * update_interval;
        if step && self.clock.is_live() {
            self.reference.pending_step = true;
            self.clock
                .accumulate(now, est_freq, offset, correction_rate);
            self.event(
                now,
                format!(
                    "Clock off by {:+.6} s, slewing: no step while live",
                    -offset
                ),
            );
        } else if step {
            self.clock.accumulate(now, est_freq, 0.0, correction_rate);
            self.clock.step(now, offset);
            self.counters.steps += 1;
            self.event(now, format!("Clock stepped by {:+.6} s", -offset));
        } else {
            self.clock
                .accumulate(now, est_freq, offset, correction_rate);
        }

        self.update_leap(now, voted_leap);

        let r = &mut self.reference;
        if r.avg2_moving {
            r.avg2_offset += 0.1 * (offset * offset - r.avg2_offset);
        } else {
            if r.avg2_offset > 0.0 && r.avg2_offset < offset * offset {
                r.avg2_moving = true;
            }
            r.avg2_offset = offset * offset;
        }

        // Error model published with the mapping.
        let dist_rate = r.skew + r.residual_freq.abs() + self.cfg.max_clock_error;
        let (ref_time, dist_base, est_base, est_rate) = (
            r.ref_time,
            r.root_delay / 2.0 + r.root_dispersion,
            r.offset_sd,
            r.frequency_sd,
        );
        let dyn_leap = r.dyn_leap;
        let snap = self.clock.snapshot_mut();
        snap.ref_host_ns = ref_time;
        snap.dist_base_s = dist_base;
        snap.dist_rate = dist_rate;
        snap.est_base_s = est_base;
        snap.est_rate = est_rate;
        snap.dyn_leap_utc_s = dyn_leap.utc_s;
        snap.dyn_leap_offset = dyn_leap.offset;

        let line = format!(
            "{} {:<15} {:2} {:10.3} {:10.3} {:10.3e} {} {:2} {:10.3e} {:10.3e} {:10.3e} {:10.3e} {:10.3e}",
            self.date(now),
            self.sources[k].addr.ip(),
            self.reference.stratum,
            self.clock.base_freq() * 1e6,
            self.reference.skew * 1e6,
            offset,
            match self.reference.leap {
                LeapStatus::Normal => 'N',
                LeapStatus::Insert => '+',
                LeapStatus::Delete => '-',
                LeapStatus::Unsynchronised => '?',
            },
            combined,
            td.offset_sd,
            remaining,
            self.reference.root_delay,
            self.root_dispersion_at(now),
            orig_root_distance + remaining.abs(),
        );
        self.log.push(LogKind::Tracking, line);
        if first {
            self.event(now, "Clock synchronised".into());
        }
    }

    /// chrony `update_leap_status`: the leap table first, then a leap the
    /// sources vote for (applied only on June 30 / December 31).
    fn update_leap(&mut self, now: i64, voted: LeapStatus) {
        let utc_s = self.utc_ns(now).div_euclid(NS);
        let r = &mut self.reference;
        let table = leap::leap_at_end_of_day(utc_s, r.dyn_leap);
        let wanted = match voted {
            LeapStatus::Insert => 1,
            LeapStatus::Delete => -1,
            _ => table,
        };
        if wanted != 0 && leap::is_leap_second_day(utc_s) && !leap::is_leap_close(utc_s, r.dyn_leap)
        {
            if table != wanted {
                let at = leap::next_midnight(utc_s);
                let offset = leap::tai_utc_at_utc(at - 1, r.dyn_leap) + wanted;
                r.dyn_leap = DynamicLeap { utc_s: at, offset };
                self.event(
                    now,
                    format!("Leap second announced by the sources: TAI - UTC becomes {offset} s"),
                );
            }
            self.reference.leap = if wanted > 0 {
                LeapStatus::Insert
            } else {
                LeapStatus::Delete
            };
        } else {
            self.reference.leap = LeapStatus::Normal;
        }
    }

    fn set_unsynchronised(&mut self, now: i64) {
        if self.reference.synchronised {
            self.event(now, "Clock unsynchronised".into());
        }
        self.reference.synchronised = false;
    }

    // ------------------------------------------------------------------
    // Helpers and reports
    // ------------------------------------------------------------------

    fn utc_ns(&self, now: i64) -> i64 {
        leap::tai_to_utc_ns(self.clock.tai_ns(now), self.reference.dyn_leap)
    }

    fn date(&self, now: i64) -> String {
        leap::format_utc(self.utc_ns(now).div_euclid(NS))
    }

    fn event(&mut self, now: i64, text: String) {
        let line = format!("{} {}", self.date(now), text);
        self.log.push(LogKind::Event, line);
    }

    pub fn sources_report(&self, now: i64) -> Vec<super::report::SourceReport> {
        // Offsets are measured against the raw timescale; report them
        // against the disciplined clock, as chrony reports slewed offsets.
        let adj = |h: i64| secs(self.clock.tai_ns(h) - (h + self.raw_base));
        self.sources
            .iter()
            .map(|s| {
                let (span, est_offset, est_err, freq, skew, sd) = s.stats.sourcestats(now);
                let last = s.last.map(|l| LastMeasurement {
                    offset: l.offset - adj(l.host_ns),
                    ..l
                });
                super::report::SourceReport {
                    id: s.id,
                    name: s.name.clone(),
                    addr: s.addr,
                    status: s.status,
                    state: s.status.chronyc_char(),
                    stratum: s.stratum,
                    poll: s.local_poll,
                    reach: s.reach,
                    last_rx_ago_s: s.last.map(|l| secs(now - l.host_ns)),
                    last,
                    samples: s.stats.n_samples(),
                    runs: s.stats.n_runs(),
                    span_s: span,
                    est_offset_s: -est_offset - adj(now),
                    est_offset_err_s: est_err,
                    freq_ppm: -freq * 1e6,
                    skew_ppm: skew * 1e6,
                    std_dev_s: sd,
                    selected: Some(s.id) == self.selected,
                    sent: s.total_tx,
                    received: s.total_rx,
                }
            })
            .collect()
    }

    pub fn tracking_report(&self, now: i64) -> super::report::TrackingReport {
        let r = &self.reference;
        let snap = self.snapshot(now);
        let synced = snap.synced;
        super::report::TrackingReport {
            synced,
            ever_synced: r.last_update.is_some(),
            ref_addr: r.ref_addr,
            ref_name: r.ref_name.clone(),
            ref_id: r
                .ref_source
                .and_then(|id| self.sources.iter().find(|s| s.id == id))
                .map(|s| s.ref_id),
            stratum: if r.last_update.is_some() {
                r.stratum
            } else {
                0
            },
            ref_time_utc_ns: r
                .last_update
                .map(|_| leap::tai_to_utc_ns(snap.tai_ns(r.ref_time), r.dyn_leap)),
            system_offset_s: -self.clock.remaining(now),
            last_offset_s: r.last_offset,
            rms_offset_s: r.avg2_offset.sqrt(),
            freq_ppm: self.clock.base_freq() * 1e6,
            resid_freq_ppm: if r.last_update.is_some() {
                r.residual_freq * 1e6
            } else {
                0.0
            },
            skew_ppm: if r.last_update.is_some() {
                r.skew * 1e6
            } else {
                0.0
            },
            root_delay_s: r.root_delay,
            root_dispersion_s: self.root_dispersion_at(now),
            update_interval_s: r.last_update_interval,
            update_age_s: r.last_update.map(|t| secs(now - t)),
            leap: if r.last_update.is_some() {
                r.leap
            } else {
                LeapStatus::Unsynchronised
            },
            tai_utc_s: leap::tai_utc_at_tai(snap.tai_ns(now), r.dyn_leap),
            est_error_s: if r.last_update.is_some() {
                secs(snap.est_error_ns(now))
            } else {
                16.0
            },
            max_error_s: if r.last_update.is_some() {
                secs(snap.max_error_ns(now))
            } else {
                16.0
            },
            combined: r.combined,
            live: self.clock.is_live(),
            pending_step: r.pending_step,
        }
    }

    /// Host ns to TAI ns with the current mapping.
    pub fn tai_ns(&self, host_ns: i64) -> i64 {
        self.clock.tai_ns(host_ns)
    }
}
