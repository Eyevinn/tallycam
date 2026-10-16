//! Deterministic network simulation around the [`Engine`]: NTP servers with
//! their own offsets, drift, delays, jitter, loss and Kiss-o'-Death codes,
//! and a host oscillator with a frequency error that can change (a
//! temperature step). Used by the unit tests and the shared scenario vectors
//! in `core/tests/vectors/`.

use core::net::{IpAddr, Ipv4Addr, SocketAddr};

use serde::Deserialize;

use super::config::{ClockConfig, SourceConfig};
use super::engine::{Action, Engine, PathInfo, RandomSource, SourceId, SplitMix64};
use super::leap::{self, DynamicLeap};
use super::ntp::{NtpPacket, NtpTimestamp, HEADER_LEN, MODE_SERVER};

const NS: i64 = 1_000_000_000;

/// One simulated NTP server.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SimServer {
    /// Pool it answers for (index into the scenario's pools).
    pub pool: usize,
    /// Server clock minus true time, ms.
    pub offset_ms: f64,
    /// Server clock frequency error, ppm.
    pub freq_ppm: f64,
    pub stratum: u8,
    /// One-way base delays, ms.
    pub up_ms: f64,
    pub down_ms: f64,
    /// Mean of the exponential queueing delay added per direction, ms.
    pub jitter_ms: f64,
    /// Probability that a request or its reply is lost.
    pub loss: f64,
    pub root_delay_ms: f64,
    pub root_dispersion_ms: f64,
    /// Answer every request with this Kiss-o'-Death code.
    pub kod: Option<String>,
    /// Leap indicator to send (0 normal, 1 insert, 2 delete).
    pub leap: u8,
    /// Does not answer at all.
    pub dead: bool,
}

impl Default for SimServer {
    fn default() -> Self {
        SimServer {
            pool: 0,
            offset_ms: 0.0,
            freq_ppm: 0.0,
            stratum: 2,
            up_ms: 10.0,
            down_ms: 10.0,
            jitter_ms: 1.0,
            loss: 0.0,
            root_delay_ms: 5.0,
            root_dispersion_ms: 2.0,
            kod: None,
            leap: 0,
            dead: false,
        }
    }
}

/// A timed event in a scenario.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SimEvent {
    /// Network path change: new delays for every server.
    PathChange {
        at_s: f64,
        key: String,
        up_ms: f64,
        down_ms: f64,
        jitter_ms: f64,
    },
    /// Host oscillator changes frequency (temperature).
    HostFreq {
        at_s: f64,
        freq_ppm: f64,
    },
    SetLive {
        at_s: f64,
        live: bool,
    },
    /// One server changes its offset (becomes a falseticker).
    ServerOffset {
        at_s: f64,
        server: usize,
        offset_ms: f64,
    },
    /// All servers stop answering (or answer again).
    AllDead {
        at_s: f64,
        dead: bool,
    },
}

impl SimEvent {
    fn at(&self) -> f64 {
        match self {
            SimEvent::PathChange { at_s, .. }
            | SimEvent::HostFreq { at_s, .. }
            | SimEvent::SetLive { at_s, .. }
            | SimEvent::ServerOffset { at_s, .. }
            | SimEvent::AllDead { at_s, .. } => *at_s,
        }
    }
}

/// A whole scenario, as stored in the vector files.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Scenario {
    pub name: String,
    /// (name, maxsources) per pool; default is Strom's chrony.conf.
    pub pools: Vec<(String, usize)>,
    pub min_sources: Option<usize>,
    pub servers: Vec<SimServer>,
    /// Host oscillator frequency error, ppm.
    pub host_freq_ppm: f64,
    /// Platform wall clock minus true UTC at start, ms.
    pub wall_error_ms: f64,
    pub duration_s: f64,
    pub live_from_start: bool,
    pub events: Vec<SimEvent>,
    pub seed: u64,
    /// True UTC at the start, Unix seconds (default 2026-10-03).
    pub start_utc_s: i64,
    pub expect: Expect,
}

/// What a scenario must show.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Expect {
    pub synced: Option<bool>,
    /// |mapped TAI - true TAI| at the end, ms.
    pub max_final_error_ms: Option<f64>,
    /// Max |error| over the last half of the run, ms.
    pub max_late_error_ms: Option<f64>,
    /// The reported max error must bound the real error at the end.
    pub max_error_bounds_error: bool,
    /// Server indices that must end as falsetickers.
    pub falsetickers: Vec<usize>,
    /// Server indices whose state must be `*` or `+` (used for sync).
    pub used: Vec<usize>,
    pub min_combined: Option<usize>,
    pub max_combined: Option<usize>,
    /// Frequency the clock must converge to, ppm (host slow positive).
    pub freq_ppm: Option<f64>,
    pub freq_tolerance_ppm: Option<f64>,
    /// No step may happen (mapped time continuous throughout).
    pub no_step: bool,
    /// Steps expected (exact).
    pub steps: Option<u64>,
    /// Minimum seconds between two non-burst requests to one server.
    pub min_poll_interval_s: Option<f64>,
    /// At most this many requests per server over the run.
    pub max_requests_per_server: Option<u64>,
    /// Servers that must have stopped receiving requests (KoD DENY).
    pub abandoned: Vec<usize>,
    /// (server, max requests) for single servers.
    pub max_requests: Vec<(usize, u64)>,
    /// Leap status at the end: normal, insert, delete.
    pub leap: Option<String>,
    /// TAI - UTC at the end of the run.
    pub tai_utc_s: Option<i32>,
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            name: String::new(),
            pools: vec![],
            min_sources: None,
            servers: vec![],
            host_freq_ppm: 0.0,
            wall_error_ms: 0.0,
            duration_s: 600.0,
            live_from_start: false,
            events: vec![],
            seed: 1,
            start_utc_s: 1_791_000_000,
            expect: Expect::default(),
        }
    }
}

/// Piecewise linear host oscillator: host ns as a function of true ns.
#[derive(Debug, Clone)]
struct HostOsc {
    /// (true ns, host ns, freq) segment starts.
    segs: Vec<(i64, i64, f64)>,
}

impl HostOsc {
    fn host(&self, t: i64) -> i64 {
        let (t0, h0, f) = *self
            .segs
            .iter()
            .rev()
            .find(|s| s.0 <= t)
            .unwrap_or(&self.segs[0]);
        h0 + (t - t0) + ((t - t0) as f64 * f).round() as i64
    }

    fn truth(&self, h: i64) -> i64 {
        let (t0, h0, f) = *self
            .segs
            .iter()
            .rev()
            .find(|s| s.1 <= h)
            .unwrap_or(&self.segs[0]);
        t0 + ((h - h0) as f64 / (1.0 + f)).round() as i64
    }

    fn set_freq(&mut self, t: i64, f: f64) {
        let h = self.host(t);
        self.segs.push((t, h, f));
    }
}

#[derive(Debug, Clone)]
struct InFlight {
    deliver_true: i64,
    source: SourceId,
    data: [u8; HEADER_LEN],
}

/// Per-server request log.
#[derive(Debug, Clone, Default)]
pub struct ServerLog {
    pub requests: Vec<i64>,
}

pub struct Simulation {
    pub engine: Engine,
    pub scenario: Scenario,
    servers: Vec<SimServer>,
    ips: Vec<IpAddr>,
    osc: HostOsc,
    /// True TAI ns now.
    pub now_true: i64,
    start_true: i64,
    rng: SplitMix64,
    in_flight: Vec<InFlight>,
    /// Which source id talks to which server.
    pub server_logs: Vec<ServerLog>,
    events: Vec<SimEvent>,
    /// (true ns, mapped - true ns) samples, once per simulated second.
    pub errors: Vec<(i64, i64)>,
    last_mapped: Option<i64>,
    /// Largest backwards or forward jump of the mapping between checks, ns.
    pub max_jump_ns: i64,
    path_key: String,
}

impl Simulation {
    pub fn new(scenario: Scenario) -> Self {
        let mut cfg = ClockConfig::default();
        if !scenario.pools.is_empty() {
            cfg.sources = scenario
                .pools
                .iter()
                .map(|(n, m)| SourceConfig::pool(n, *m))
                .collect();
        }
        if let Some(m) = scenario.min_sources {
            cfg.min_sources = m;
        }
        let start_true = leap::utc_to_tai_ns(scenario.start_utc_s * NS, DynamicLeap::default());
        let osc = HostOsc {
            segs: vec![(start_true, 500 * NS, scenario.host_freq_ppm * 1e-6)],
        };
        let h0 = osc.host(start_true);
        let wall = leap::tai_to_utc_ns(start_true, DynamicLeap::default())
            + (scenario.wall_error_ms * 1e6) as i64;
        let engine = Engine::new(cfg, h0, wall, Box::new(SplitMix64(scenario.seed ^ 0xABCD)));
        let ips = (0..scenario.servers.len())
            .map(|i| IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10 + i as u8)))
            .collect();
        let mut events = scenario.events.clone();
        events.sort_by(|a, b| a.at().total_cmp(&b.at()));
        let mut sim = Simulation {
            engine,
            servers: scenario.servers.clone(),
            ips,
            osc,
            now_true: start_true,
            start_true,
            rng: SplitMix64(scenario.seed),
            in_flight: Vec::new(),
            server_logs: vec![ServerLog::default(); scenario.servers.len()],
            events,
            errors: Vec::new(),
            last_mapped: None,
            max_jump_ns: 0,
            path_key: "sim0".into(),
            scenario,
        };
        sim.engine.on_path_change(
            h0,
            PathInfo {
                satisfied: true,
                ipv4: true,
                ipv6: true,
                key: "sim0".into(),
            },
        );
        if sim.scenario.live_from_start {
            sim.engine.set_live(h0, true);
        }
        sim
    }

    pub fn host_now(&self) -> i64 {
        self.osc.host(self.now_true)
    }

    fn unit(&mut self) -> f64 {
        (self.rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn exp(&mut self, mean_ms: f64) -> f64 {
        -mean_ms * (1.0 - self.unit()).ln()
    }

    fn server_index(&self, addr: &SocketAddr) -> Option<usize> {
        self.ips.iter().position(|ip| *ip == addr.ip())
    }

    fn server_clock(&self, i: usize, t: i64) -> i64 {
        let s = &self.servers[i];
        t + (s.offset_ms * 1e6) as i64 + ((t - self.start_true) as f64 * s.freq_ppm * 1e-6) as i64
    }

    fn handle(&mut self, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Resolve { config_index, .. } => {
                    let addrs: Vec<IpAddr> = (0..self.servers.len())
                        .filter(|&i| self.servers[i].pool == config_index)
                        .map(|i| self.ips[i])
                        .collect();
                    let h = self.host_now();
                    self.engine.on_resolved(h, config_index, Ok(addrs));
                }
                Action::Send {
                    source,
                    addr,
                    packet,
                } => {
                    let h = self.host_now();
                    self.engine.on_transmitted(source, &packet, h);
                    let Some(i) = self.server_index(&addr) else {
                        continue;
                    };
                    self.server_logs[i].requests.push(self.now_true);
                    let srv = self.servers[i].clone();
                    if srv.dead || self.unit() < srv.loss {
                        continue;
                    }
                    let up = srv.up_ms + self.exp(srv.jitter_ms);
                    let down = srv.down_ms + self.exp(srv.jitter_ms);
                    let arrive = self.now_true + (up * 1e6) as i64;
                    let depart = arrive + 30_000;
                    let req = NtpPacket::decode(&packet).unwrap();
                    let utc = |t: i64| leap::tai_to_utc_ns(t, DynamicLeap::default());
                    let mut reply = NtpPacket {
                        leap: srv.leap,
                        version: 4,
                        mode: MODE_SERVER,
                        stratum: srv.stratum,
                        poll: req.poll,
                        precision: -24,
                        root_delay: (srv.root_delay_ms * 65.536) as u32,
                        root_dispersion: (srv.root_dispersion_ms * 65.536) as u32,
                        reference_id: 0x7F7F_0001,
                        reference_ts: NtpTimestamp::from_unix_ns(
                            utc(self.server_clock(i, arrive)) - 10 * NS,
                        ),
                        origin_ts: req.transmit_ts,
                        receive_ts: NtpTimestamp::from_unix_ns(utc(self.server_clock(i, arrive))),
                        transmit_ts: NtpTimestamp::from_unix_ns(utc(self.server_clock(i, depart))),
                    };
                    if let Some(code) = &srv.kod {
                        let mut b = [b' '; 4];
                        b.copy_from_slice(&code.as_bytes()[..4]);
                        reply.leap = 3;
                        reply.stratum = 0;
                        reply.reference_id = u32::from_be_bytes(b);
                    }
                    if self.unit() < srv.loss {
                        continue;
                    }
                    self.in_flight.push(InFlight {
                        deliver_true: depart + (down * 1e6) as i64,
                        source,
                        data: reply.encode(),
                    });
                }
            }
        }
    }

    fn apply_event(&mut self, e: SimEvent) {
        let h = self.host_now();
        match e {
            SimEvent::PathChange {
                key,
                up_ms,
                down_ms,
                jitter_ms,
                ..
            } => {
                for s in self.servers.iter_mut() {
                    s.up_ms = up_ms;
                    s.down_ms = down_ms;
                    s.jitter_ms = jitter_ms;
                }
                self.path_key = key.clone();
                self.engine.on_path_change(
                    h,
                    PathInfo {
                        satisfied: true,
                        ipv4: true,
                        ipv6: true,
                        key,
                    },
                );
            }
            SimEvent::HostFreq { freq_ppm, .. } => {
                let t = self.now_true;
                self.osc.set_freq(t, freq_ppm * 1e-6);
            }
            SimEvent::SetLive { live, .. } => self.engine.set_live(h, live),
            SimEvent::ServerOffset {
                server, offset_ms, ..
            } => {
                if let Some(s) = self.servers.get_mut(server) {
                    s.offset_ms = offset_ms;
                }
            }
            SimEvent::AllDead { dead, .. } => {
                for s in self.servers.iter_mut() {
                    s.dead = dead;
                }
            }
        }
    }

    /// Runs until `duration_s` of true time has passed since the start.
    pub fn run_until(&mut self, at_s: f64) {
        let end = self.start_true + (at_s * 1e9) as i64;
        let mut next_check = self.now_true;
        loop {
            let h = self.host_now();
            let actions = self.engine.poll(h);
            self.handle(actions);
            let wake_true = self.osc.truth(self.engine.next_wake(self.host_now()));
            let pkt_true = self
                .in_flight
                .iter()
                .map(|p| p.deliver_true)
                .min()
                .unwrap_or(i64::MAX);
            let ev_true = self
                .events
                .first()
                .map(|e| self.start_true + (e.at() * 1e9) as i64)
                .unwrap_or(i64::MAX);
            let next = wake_true
                .min(pkt_true)
                .min(ev_true)
                .min(next_check)
                .min(end)
                .max(self.now_true + 1);
            self.now_true = next;
            if self.now_true >= next_check {
                self.check_mapping();
                next_check += NS;
            }
            if self.now_true >= ev_true && !self.events.is_empty() {
                let e = self.events.remove(0);
                self.apply_event(e);
            }
            let due: Vec<InFlight> = {
                let (due, rest): (Vec<_>, Vec<_>) = self
                    .in_flight
                    .drain(..)
                    .partition(|p| p.deliver_true <= self.now_true);
                self.in_flight = rest;
                due
            };
            for p in due {
                let h4 = self.osc.host(p.deliver_true);
                self.engine.on_packet(h4, p.source, &p.data);
            }
            if self.now_true >= end {
                break;
            }
        }
    }

    fn check_mapping(&mut self) {
        let h = self.host_now();
        let mapped = self.engine.tai_ns(h);
        self.errors.push((self.now_true, mapped - self.now_true));
        if let Some(prev) = self.last_mapped {
            // Checked once per true second: the mapped advance must be 1 s
            // within the slew bound (plus the host frequency error).
            let jump = (mapped - prev - NS).abs();
            self.max_jump_ns = self.max_jump_ns.max(jump);
        }
        self.last_mapped = Some(mapped);
    }

    /// Mapped minus true TAI now, ns.
    pub fn error_ns(&self) -> i64 {
        self.engine.tai_ns(self.host_now()) - self.now_true
    }

    /// Which server a source id talks to.
    pub fn server_of(&self, addr: &SocketAddr) -> Option<usize> {
        self.server_index(addr)
    }

    pub fn run(&mut self) {
        let d = self.scenario.duration_s;
        self.run_until(d);
    }

    /// Checks the scenario's expectations; returns the failures.
    pub fn check(&self) -> Vec<String> {
        let e = &self.scenario.expect;
        let mut fails = Vec::new();
        let now = self.host_now();
        let t = self.engine.tracking_report(now);
        let sources = self.engine.sources_report(now);
        let state_of = |srv: usize| {
            sources
                .iter()
                .find(|s| self.server_index(&s.addr) == Some(srv))
                .map(|s| s.state)
        };
        if let Some(want) = e.synced {
            if t.synced != want {
                fails.push(format!("synced {} != {want}", t.synced));
            }
        }
        let err_ms = self.error_ns() as f64 / 1e6;
        if let Some(max) = e.max_final_error_ms {
            if err_ms.abs() > max {
                fails.push(format!("final error {err_ms:.3} ms > {max} ms"));
            }
        }
        if let Some(max) = e.max_late_error_ms {
            let half = self.start_true + (self.scenario.duration_s * 0.5e9) as i64;
            let worst = self
                .errors
                .iter()
                .filter(|(t, _)| *t >= half)
                .map(|(_, e)| (*e as f64 / 1e6).abs())
                .fold(0.0, f64::max);
            if worst > max {
                fails.push(format!("late error {worst:.3} ms > {max} ms"));
            }
        }
        if e.max_error_bounds_error && err_ms.abs() > t.max_error_s * 1e3 {
            fails.push(format!(
                "max error {:.3} ms does not bound the error {err_ms:.3} ms",
                t.max_error_s * 1e3
            ));
        }
        for &srv in &e.falsetickers {
            if state_of(srv) != Some('x') {
                fails.push(format!(
                    "server {srv} state {:?}, expected falseticker",
                    state_of(srv)
                ));
            }
        }
        for &srv in &e.used {
            if !matches!(state_of(srv), Some('*') | Some('+')) {
                fails.push(format!(
                    "server {srv} state {:?}, expected * or +",
                    state_of(srv)
                ));
            }
        }
        let combined = sources
            .iter()
            .filter(|s| s.state == '*' || s.state == '+')
            .count();
        if let Some(min) = e.min_combined {
            if combined < min {
                fails.push(format!("{combined} sources used, expected at least {min}"));
            }
        }
        if let Some(max) = e.max_combined {
            if combined > max {
                fails.push(format!("{combined} sources used, expected at most {max}"));
            }
        }
        if let Some(f) = e.freq_ppm {
            let tol = e.freq_tolerance_ppm.unwrap_or(0.5);
            if (t.freq_ppm - f).abs() > tol {
                fails.push(format!(
                    "frequency {:.3} ppm, expected {f} +- {tol}",
                    t.freq_ppm
                ));
            }
        }
        // Mapping continuity: per simulated second the mapped clock may differ
        // from 1 s only by the slew bound and the host frequency error.
        let always_live = self.scenario.live_from_start
            && !self
                .scenario
                .events
                .iter()
                .any(|e| matches!(e, SimEvent::SetLive { .. }));
        let slew = if always_live {
            self.engine.config().max_slew_live
        } else {
            self.engine.config().max_slew_idle
        };
        let bound = (slew * 1e9) as i64 + 200_000;
        if e.no_step && self.max_jump_ns > bound {
            fails.push(format!(
                "mapping jumped {} ns in one second",
                self.max_jump_ns
            ));
        }
        if let Some(steps) = e.steps {
            if self.engine.counters().steps != steps {
                fails.push(format!(
                    "{} steps, expected {steps}",
                    self.engine.counters().steps
                ));
            }
        }
        if let Some(min) = e.min_poll_interval_s {
            for (i, log) in self.server_logs.iter().enumerate() {
                // Skip the iburst: requests closer than 3 s are burst requests.
                let gaps: Vec<f64> = log
                    .requests
                    .windows(2)
                    .map(|w| (w[1] - w[0]) as f64 / 1e9)
                    .collect();
                for g in gaps.iter().filter(|g| **g > 3.0) {
                    if *g < min {
                        fails.push(format!("server {i}: poll gap {g:.1} s < {min} s"));
                    }
                }
            }
        }
        if let Some(max) = e.max_requests_per_server {
            for (i, log) in self.server_logs.iter().enumerate() {
                if log.requests.len() as u64 > max {
                    fails.push(format!(
                        "server {i}: {} requests > {max}",
                        log.requests.len()
                    ));
                }
            }
        }
        for &(srv, max) in &e.max_requests {
            let n = self
                .server_logs
                .get(srv)
                .map_or(0, |l| l.requests.len() as u64);
            if n > max {
                fails.push(format!("server {srv}: {n} requests > {max}"));
            }
        }
        if let Some(want) = &e.leap {
            if t.leap.name() != want {
                fails.push(format!("leap {} != {want}", t.leap.name()));
            }
        }
        if let Some(want) = e.tai_utc_s {
            if t.tai_utc_s != want {
                fails.push(format!("TAI - UTC {} != {want}", t.tai_utc_s));
            }
        }
        for &srv in &e.abandoned {
            let last = self.server_logs[srv].requests.last().copied().unwrap_or(0);
            if last > self.start_true + (self.scenario.duration_s * 0.5e9) as i64 {
                fails.push(format!("server {srv} still polled after KoD"));
            }
        }
        fails
    }
}
