//! clockd-probe: the TallyCam clock discipline on this machine.
//!
//! Runs the same core the phone app uses against the real NTP pools (Strom's
//! chrony.conf by default) and prints `chronyc sources`, `sourcestats` and
//! `tracking` style reports, plus the disciplined time against this
//! machine's own system clock.
//!
//! Usage: clockd-probe [--minutes N] [--interval S] [--conf chrony.conf]
//!                     [--log FILE] [--driftfile FILE] [--system] [--live]

use std::sync::Arc;
use std::time::{Duration, Instant};

use tallycam_core::clock::driver::{
    default_host_clock, DisciplineDriver, DriverOptions, FileLogSink, LogSink, StdResolver,
    SystemClockDriver,
};
use tallycam_core::clock::{ClockConfig, ClockSource, SnapshotCell};

struct Args {
    minutes: f64,
    interval: u64,
    conf: Option<String>,
    log: Option<String>,
    system: bool,
    live: bool,
    drift: Option<String>,
}

fn args() -> Result<Args, String> {
    let mut a = Args {
        minutes: 10.0,
        interval: 30,
        conf: None,
        log: None,
        system: false,
        live: false,
        drift: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--minutes" => a.minutes = val("--minutes")?.parse().map_err(|_| "bad --minutes")?,
            "--interval" => {
                a.interval = val("--interval")?.parse().map_err(|_| "bad --interval")?
            }
            "--conf" => a.conf = Some(val("--conf")?),
            "--log" => a.log = Some(val("--log")?),
            "--driftfile" => a.drift = Some(val("--driftfile")?),
            "--system" => a.system = true,
            "--live" => a.live = true,
            "-h" | "--help" => {
                println!(
                    "clockd-probe [--minutes N] [--interval S] [--conf chrony.conf] [--log FILE] [--driftfile FILE] [--system] [--live]\n\n\
                     --system  report the OS clock (for hosts running chrony) instead of disciplining\n\
                     --live    run as while streaming: no steps, slew at most 500 ppm"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

fn main() {
    let a = match args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let host = default_host_clock();
    let cell = Arc::new(SnapshotCell::new());
    let source: Arc<dyn ClockSource> = if a.system {
        SystemClockDriver::start(host.clone(), cell.clone())
    } else {
        let cfg = match &a.conf {
            Some(path) => {
                let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
                ClockConfig::from_chrony_conf(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
            }
            None => ClockConfig::default(),
        };
        let sink: Option<Box<dyn LogSink>> = a
            .log
            .as_ref()
            .map(|p| Box::new(FileLogSink::new(p.into(), 8 << 20)) as Box<dyn LogSink>);
        let options = DriverOptions {
            log_sink: sink,
            drift_file: a.drift.as_ref().map(Into::into),
        };
        DisciplineDriver::start(
            cfg,
            host.clone(),
            Arc::new(StdResolver),
            options,
            cell.clone(),
        )
    };
    source.set_live(a.live);

    let start = Instant::now();
    let end = Duration::from_secs_f64(a.minutes * 60.0);
    let mut next = Duration::from_secs(a.interval);
    let mut diffs: Vec<f64> = Vec::new();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= end {
            break;
        }
        std::thread::sleep(
            next.saturating_sub(elapsed)
                .min(end.saturating_sub(elapsed)),
        );
        let elapsed = start.elapsed();
        // Disciplined UTC against the system clock, read back to back.
        let h = host.now_ns();
        let sys = host.wall_utc_ns();
        let h2 = host.now_ns();
        let m = source.map((h + h2) / 2).expect("mapping published");
        let diff_ms = (m.utc_ns - sys) as f64 / 1e6;
        println!("==== t = {:>5.0} s ====", elapsed.as_secs_f64());
        print!("{}", source.reports_text());
        println!(
            "Disciplined - system clock: {:+.3} ms (synced {}, est. error {:.3} ms, max error {:.3} ms)\n",
            diff_ms,
            m.synced,
            m.est_error_ns as f64 / 1e6,
            m.max_error_ns as f64 / 1e6
        );
        if m.synced {
            diffs.push(diff_ms);
        }
        next += Duration::from_secs(a.interval);
    }
    let status = source.status();
    println!("==== summary ====");
    println!("{}", serde_json_line(&status));
    if !diffs.is_empty() {
        let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
        let min = diffs.iter().copied().fold(f64::MAX, f64::min);
        let max = diffs.iter().copied().fold(f64::MIN, f64::max);
        println!("Disciplined - system clock while synced: mean {mean:+.3} ms, min {min:+.3} ms, max {max:+.3} ms over {} reports", diffs.len());
    }
    if !a.system {
        let tail = source.log_tail(&[tallycam_core::clock::log::LogKind::Event], 50);
        println!("Events:\n  {}", tail.join("\n  "));
    }
}

fn serde_json_line(status: &tallycam_core::protocol::ClockStatus) -> String {
    // The LCP status.clock object as the phone sends it.
    tallycam_core::clock::report::to_json(status)
}
