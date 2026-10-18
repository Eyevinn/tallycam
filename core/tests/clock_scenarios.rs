//! Runs the shared clock scenario vectors (`tests/vectors/clock_scenarios.json`)
//! through the simulated network, and checks engine details the vectors do
//! not express.

use std::net::SocketAddr;

use serde::Deserialize;
use tallycam_core::clock::engine::{Action, Engine, SplitMix64};
use tallycam_core::clock::mapping::MAX_ERROR_CAP_NS;
use tallycam_core::clock::ntp::{NtpPacket, NtpTimestamp, MODE_SERVER};
use tallycam_core::clock::sim::{Scenario, SimServer, Simulation};
use tallycam_core::clock::{leap, ClockConfig, SourceConfig};

#[derive(Deserialize)]
struct Vectors {
    scenarios: Vec<Scenario>,
}

fn vectors() -> Vec<Scenario> {
    let text = include_str!("vectors/clock_scenarios.json");
    serde_json::from_str::<Vectors>(text)
        .expect("vector file parses")
        .scenarios
}

#[test]
fn scenario_vectors() {
    let mut failures = Vec::new();
    for sc in vectors() {
        let name = sc.name.clone();
        let mut sim = Simulation::new(sc);
        sim.run();
        let fails = sim.check();
        let now = sim.host_now();
        let t = sim.engine.tracking_report(now);
        println!(
            "{name}: synced {} error {:.3} ms max error {:.3} ms est {:.3} ms freq {:.3} ppm skew {:.3} ppm rms {:.3} ms steps {}",
            t.synced,
            sim.error_ns() as f64 / 1e6,
            t.max_error_s * 1e3,
            t.est_error_s * 1e3,
            t.freq_ppm,
            t.skew_ppm,
            t.rms_offset_s * 1e3,
            sim.engine.counters().steps
        );
        if !fails.is_empty() {
            println!(
                "{}",
                tallycam_core::clock::report::format_sources(&sim.engine.sources_report(now))
            );
            println!("{}", tallycam_core::clock::report::format_tracking(&t));
            failures.push(format!("{name}: {}", fails.join("; ")));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

fn scenario(servers: Vec<SimServer>) -> Scenario {
    Scenario {
        name: "t".into(),
        pools: vec![("pool.example.com".into(), servers.len())],
        servers,
        duration_s: 600.0,
        ..Default::default()
    }
}

fn good_servers(n: usize) -> Vec<SimServer> {
    (0..n)
        .map(|i| SimServer {
            offset_ms: 0.05 * i as f64,
            ..Default::default()
        })
        .collect()
}

#[test]
fn mapped_time_is_monotonic_with_bounded_slew_while_live() {
    let mut sc = scenario(good_servers(4));
    sc.wall_error_ms = 400.0;
    sc.live_from_start = true;
    let mut sim = Simulation::new(sc);
    // Map 25 fps frame times during the first minutes, while 400 ms are slewed in.
    let mut last: Option<(i64, i64)> = None;
    for step in 0..(120 * 25) {
        sim.run_until(step as f64 / 25.0);
        let h = sim.host_now();
        let m = sim.engine.tai_ns(h);
        if let Some((lh, lm)) = last {
            let dh = h - lh;
            let dm = m - lm;
            assert!(dm > 0, "mapped time ran backwards");
            // At most 500 ppm (plus rounding) away from the host interval.
            assert!(
                (dm - dh).abs() as f64 <= dh as f64 * 500e-6 + 2.0,
                "frame interval {dm} vs {dh}"
            );
        }
        last = Some((h, m));
    }
    assert_eq!(sim.engine.counters().steps, 0);
}

#[test]
fn max_error_grows_between_updates_and_unsyncs_in_long_holdover() {
    let mut sc = scenario(good_servers(4));
    sc.duration_s = 900.0;
    let mut sim = Simulation::new(sc);
    sim.run_until(900.0);
    let now = sim.host_now();
    assert!(sim.engine.is_synchronised(now));
    let snap = sim.engine.snapshot(now);
    let e0 = snap.max_error_ns(now);
    let e1 = snap.max_error_ns(now + 100_000_000_000);
    let e2 = snap.max_error_ns(now + 200_000_000_000);
    assert!(e1 > e0 && e2 > e1, "{e0} {e1} {e2}");
    // The root distance part grows linearly (the rest is the correction
    // still being slewed in).
    let dist = |h: i64| snap.max_error_ns(h) - (snap.remaining_s(h).abs() * 1e9) as i64;
    let (d0, d1, d2) = (
        dist(now),
        dist(now + 100_000_000_000),
        dist(now + 200_000_000_000),
    );
    assert!(
        ((d2 - d1) - (d1 - d0)).abs() < 1_000,
        "linear growth {d0} {d1} {d2}"
    );
    assert!(d1 - d0 > 0);
    assert!(snap.max_error_ns(i64::MAX / 4) <= MAX_ERROR_CAP_NS);
    // All servers go silent: holdover, then unsynchronised.
    let mut sim2 = Simulation::new(Scenario {
        events: vec![
            serde_json::from_str(r#"{"kind":"all_dead","at_s":900,"dead":true}"#).unwrap(),
        ],
        duration_s: 4000.0,
        ..scenario(good_servers(4))
    });
    sim2.run_until(1000.0);
    assert!(
        sim2.engine.is_synchronised(sim2.host_now()),
        "holdover keeps sync for a while"
    );
    sim2.run_until(4000.0);
    let t = sim2.engine.tracking_report(sim2.host_now());
    assert!(!t.synced, "long holdover is not synchronised");
    assert!(t.max_error_s > 0.0);
}

#[test]
fn poll_interval_stays_within_minpoll_and_maxpoll() {
    let mut sim = Simulation::new(Scenario {
        duration_s: 3600.0,
        ..scenario(good_servers(4))
    });
    sim.run();
    for log in &sim.server_logs {
        let burst: Vec<_> = log.requests.iter().take(8).collect();
        assert!(burst.len() >= 4);
        for w in log.requests.windows(2).skip(8) {
            let gap = (w[1] - w[0]) as f64 / 1e9;
            assert!((64.0..=256.0 * 1.03).contains(&gap), "gap {gap}");
        }
    }
    // Stable sources back off to maxpoll 8 (256 s), like chrony.
    let polls: Vec<i8> = sim
        .engine
        .sources_report(sim.host_now())
        .iter()
        .map(|s| s.poll)
        .collect();
    assert!(polls.iter().all(|&p| (6..=8).contains(&p)), "{polls:?}");
    assert!(polls.contains(&8), "{polls:?}");
}

/// Builds an engine with one fixed server and returns it with its first request.
fn engine_with_one_request() -> (Engine, i64, u32, SocketAddr, [u8; 48]) {
    let cfg = ClockConfig {
        sources: vec![SourceConfig::server("192.0.2.1")],
        ..Default::default()
    };
    let now = 1_000_000_000_000;
    let mut e = Engine::new(cfg, now, 1_791_000_000_000_000_000, Box::new(SplitMix64(1)));
    for a in e.poll(now) {
        if let Action::Resolve { config_index, .. } = a {
            e.on_resolved(now, config_index, Ok(vec!["192.0.2.1".parse().unwrap()]));
        }
    }
    let t = e.next_wake(now);
    let actions = e.poll(t);
    let Some(Action::Send {
        source,
        addr,
        packet,
    }) = actions.into_iter().next()
    else {
        panic!("no request")
    };
    e.on_transmitted(source, &packet, t);
    (e, t, source, addr, packet)
}

fn reply_to(packet: &[u8; 48], utc_ns: i64) -> NtpPacket {
    let req = NtpPacket::decode(packet).unwrap();
    NtpPacket {
        leap: 0,
        version: 4,
        mode: MODE_SERVER,
        stratum: 2,
        poll: 6,
        precision: -20,
        root_delay: 100,
        root_dispersion: 100,
        reference_id: 1,
        reference_ts: NtpTimestamp::from_unix_ns(utc_ns - 1_000_000_000),
        origin_ts: req.transmit_ts,
        receive_ts: NtpTimestamp::from_unix_ns(utc_ns),
        transmit_ts: NtpTimestamp::from_unix_ns(utc_ns + 10_000),
    }
}

#[test]
fn origin_cookie_rejects_spoofed_stale_and_duplicate_replies() {
    let (mut e, t1, source, _, packet) = engine_with_one_request();
    let req = NtpPacket::decode(&packet).unwrap();
    assert!(!req.transmit_ts.is_zero());
    assert!(
        req.origin_ts.is_zero() && req.receive_ts.is_zero(),
        "no local time in the request"
    );
    let utc = 1_791_000_000_000_000_000 + (t1 - 1_000_000_000_000);

    // Wrong origin: spoofed or an answer to an older request.
    let mut spoofed = reply_to(&packet, utc);
    spoofed.origin_ts = NtpTimestamp(spoofed.origin_ts.0 ^ 1);
    e.on_packet(t1 + 20_000_000, source, &spoofed.encode());
    assert_eq!(e.counters().bogus, 1);
    assert_eq!(e.counters().received, 0);

    // The genuine reply is accepted once.
    let good = reply_to(&packet, utc).encode();
    e.on_packet(t1 + 20_000_000, source, &good);
    assert_eq!(e.counters().received, 1);
    // A replay of it is not.
    e.on_packet(t1 + 21_000_000, source, &good);
    assert_eq!(e.counters().received, 1);
    assert_eq!(e.counters().bogus, 2);
    let s = &e.sources_report(t1)[0];
    assert_eq!(s.reach, 1);
    let last = s.last.unwrap();
    assert!(last.offset.abs() < 0.02, "offset {}", last.offset);
}

#[test]
fn client_mode_and_unsynchronised_server_are_not_used() {
    let (mut e, t1, source, _, packet) = engine_with_one_request();
    let utc = 1_791_000_000_000_000_000 + (t1 - 1_000_000_000_000);
    let mut r = reply_to(&packet, utc);
    r.mode = 3;
    e.on_packet(t1 + 1_000_000, source, &r.encode());
    assert_eq!(e.counters().received, 0, "only server mode replies count");
    let mut r = reply_to(&packet, utc);
    r.leap = 3;
    e.on_packet(t1 + 1_000_000, source, &r.encode());
    let s = &e.sources_report(t1)[0];
    assert_eq!(e.counters().received, 1);
    assert_eq!(
        s.reach, 0,
        "an unsynchronised server does not count as reachable"
    );
    assert!(s.last.is_none());
}

#[test]
fn kod_deny_takes_the_source_offline() {
    let (mut e, t1, source, _, packet) = engine_with_one_request();
    let mut r = reply_to(&packet, 0);
    r.leap = 3;
    r.stratum = 0;
    r.reference_id = u32::from_be_bytes(*b"DENY");
    e.on_packet(t1 + 1_000_000, source, &r.encode());
    assert_eq!(e.counters().kod, 1);
    let s = &e.sources_report(t1)[0];
    assert_eq!(s.state, '?');
    // Never polled again.
    for k in 1..20 {
        let actions = e.poll(t1 + k * 100_000_000_000);
        assert!(!actions.iter().any(|a| matches!(a, Action::Send { .. })));
    }
}

#[test]
fn announced_leap_is_ignored_past_midnight() {
    let mut sim = Simulation::new(
        vectors()
            .into_iter()
            .find(|s| s.name == "leap_second_announced")
            .expect("leap vector"),
    );
    let midnight = 1_798_761_600i64; // 2027-01-01
    sim.run();
    let now = sim.host_now();
    assert!(sim.engine.tracking_report(now).synced);
    // No leap was scheduled from the vote ...
    let snap = sim.engine.snapshot(now);
    assert_eq!(snap.dyn_leap_utc_s, 0);
    assert_eq!(snap.dyn_leap_offset, 0);
    // ... so TAI is where the servers (which never inserted it) put it, and
    // UTC an hour past midnight is TAI - 37 s.
    assert!(
        sim.error_ns().abs() < 5_000_000,
        "error {} ns",
        sim.error_ns()
    );
    let tai = sim.engine.tai_ns(now);
    let utc = leap::tai_to_utc_ns(tai, snap.dynamic_leap());
    assert_eq!(tai - utc, 37 * 1_000_000_000);
    assert!(utc.div_euclid(1_000_000_000) >= midnight + 3_590);
}
