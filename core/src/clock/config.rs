//! Discipline configuration. The default is the chrony.conf Strom installs
//! on its hosts (Ateliere Live style, `scripts/setup/ntp/install-chrony.sh`):
//!
//! ```text
//! pool ntp.ubuntu.com        iburst maxsources 4 minpoll 6 maxpoll 8
//! pool 0.ubuntu.pool.ntp.org iburst maxsources 1 minpoll 6 maxpoll 8
//! pool 1.ubuntu.pool.ntp.org iburst maxsources 1 minpoll 6 maxpoll 8
//! pool 2.ubuntu.pool.ntp.org iburst maxsources 2 minpoll 6 maxpoll 8
//! leapsectz right/UTC
//! minsources 3
//! log measurements statistics tracking selection
//! makestep 1.0 3
//! ```

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A name that resolves to many servers; up to `max_sources` are used
    /// and bad ones are replaced.
    Pool,
    /// One server (a name is resolved to one address, or an IP literal).
    Server,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceConfig {
    pub kind: SourceKind,
    pub name: String,
    pub port: u16,
    pub iburst: bool,
    pub max_sources: usize,
    pub min_poll: i8,
    pub max_poll: i8,
}

impl SourceConfig {
    pub fn pool(name: &str, max_sources: usize) -> Self {
        SourceConfig {
            kind: SourceKind::Pool,
            name: name.into(),
            port: super::ntp::PORT,
            iburst: true,
            max_sources,
            min_poll: 6,
            max_poll: 8,
        }
    }

    pub fn server(name: &str) -> Self {
        SourceConfig {
            kind: SourceKind::Server,
            max_sources: 1,
            ..Self::pool(name, 1)
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClockConfig {
    pub sources: Vec<SourceConfig>,
    /// `minsources`: sources that must agree before the clock is updated.
    pub min_sources: usize,
    /// `makestep threshold limit`.
    pub make_step_threshold: f64,
    pub make_step_limit: u32,
    /// Slew rate bound while a stream is live (fraction, 500e-6 = 500 ppm).
    pub max_slew_live: f64,
    /// Slew rate bound otherwise (chrony's default `maxslewrate` 83333 ppm).
    pub max_slew_idle: f64,
    pub max_distance: f64,
    pub max_jitter: f64,
    pub combine_limit: f64,
    pub reselect_distance: f64,
    pub stratum_weight: f64,
    pub max_stratum: u8,
    pub max_delay: f64,
    pub max_delay_dev_ratio: f64,
    pub poll_target: f64,
    /// `maxclockerror`: assumed frequency error of the host clock.
    pub max_clock_error: f64,
    pub corr_time_ratio: f64,
    pub max_update_skew: f64,
    /// Precision of host clock readings, s.
    pub precision: f64,
    /// Minimum time between bursts triggered by network path changes, s.
    pub path_burst_min_interval: f64,
    /// Lines kept in the in-memory log.
    pub log_capacity: usize,
    /// Frequency (ppm, positive = host slow) and skew (ppm) saved by an
    /// earlier run, like chrony's driftfile.
    pub drift: Option<(f64, f64)>,
}

impl Default for ClockConfig {
    fn default() -> Self {
        ClockConfig {
            sources: vec![
                SourceConfig::pool("ntp.ubuntu.com", 4),
                SourceConfig::pool("0.ubuntu.pool.ntp.org", 1),
                SourceConfig::pool("1.ubuntu.pool.ntp.org", 1),
                SourceConfig::pool("2.ubuntu.pool.ntp.org", 2),
            ],
            min_sources: 3,
            make_step_threshold: 1.0,
            make_step_limit: 3,
            max_slew_live: 500e-6,
            max_slew_idle: 83_333.333e-6,
            max_distance: 3.0,
            max_jitter: 1.0,
            combine_limit: 3.0,
            reselect_distance: 1e-4,
            stratum_weight: 1e-3,
            max_stratum: 15,
            max_delay: 3.0,
            max_delay_dev_ratio: 10.0,
            poll_target: 8.0,
            max_clock_error: 1e-6,
            corr_time_ratio: 3.0,
            max_update_skew: 1000e-6,
            precision: 1.2e-7,
            path_burst_min_interval: 120.0,
            log_capacity: 4000,
            drift: None,
        }
    }
}

impl ClockConfig {
    /// Reads the directives that matter here from chrony.conf syntax:
    /// `pool`, `server` (with `iburst`, `maxsources`, `minpoll`, `maxpoll`,
    /// `port`), `minsources` and `makestep`. Other lines are ignored.
    /// Starts from the defaults but with no sources.
    pub fn from_chrony_conf(text: &str) -> Result<Self, String> {
        let mut cfg = ClockConfig {
            sources: Vec::new(),
            ..Default::default()
        };
        for (n, line) in text.lines().enumerate() {
            let line = line.split(['#', '!', ';']).next().unwrap_or("").trim();
            let mut words = line.split_whitespace();
            let Some(directive) = words.next() else {
                continue;
            };
            let err = |what: &str| format!("line {}: {what}", n + 1);
            match directive {
                "pool" | "server" => {
                    let name = words.next().ok_or_else(|| err("missing name"))?;
                    let mut src = if directive == "pool" {
                        SourceConfig {
                            iburst: false,
                            ..SourceConfig::pool(name, 4)
                        }
                    } else {
                        SourceConfig {
                            iburst: false,
                            min_poll: 6,
                            max_poll: 10,
                            ..SourceConfig::server(name)
                        }
                    };
                    if directive == "pool" {
                        src.max_poll = 10;
                    }
                    while let Some(opt) = words.next() {
                        let mut num = |what: &str| -> Result<i64, String> {
                            words
                                .next()
                                .and_then(|v| v.parse().ok())
                                .ok_or_else(|| err(&format!("{what} needs a number")))
                        };
                        match opt {
                            "iburst" => src.iburst = true,
                            "maxsources" => {
                                src.max_sources = num("maxsources")?.clamp(1, 16) as usize
                            }
                            "minpoll" => src.min_poll = num("minpoll")?.clamp(0, 24) as i8,
                            "maxpoll" => src.max_poll = num("maxpoll")?.clamp(0, 24) as i8,
                            "port" => src.port = num("port")?.clamp(1, 65535) as u16,
                            _ => {}
                        }
                    }
                    src.max_poll = src.max_poll.max(src.min_poll);
                    cfg.sources.push(src);
                }
                "minsources" => {
                    cfg.min_sources = words
                        .next()
                        .and_then(|v| v.parse().ok())
                        .ok_or_else(|| err("minsources"))?;
                }
                "makestep" => {
                    cfg.make_step_threshold = words
                        .next()
                        .and_then(|v| v.parse().ok())
                        .ok_or_else(|| err("makestep threshold"))?;
                    let limit: i64 = words
                        .next()
                        .and_then(|v| v.parse().ok())
                        .ok_or_else(|| err("makestep limit"))?;
                    cfg.make_step_limit = limit.max(0) as u32;
                }
                _ => {}
            }
        }
        if cfg.sources.is_empty() {
            return Err("no pool or server lines".into());
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strom_chrony_conf_equals_default() {
        let conf = "\
# Ateliere Live-style chrony.conf
pool ntp.ubuntu.com        iburst maxsources 4 minpoll 6 maxpoll 8
pool 0.ubuntu.pool.ntp.org iburst maxsources 1 minpoll 6 maxpoll 8
pool 1.ubuntu.pool.ntp.org iburst maxsources 1 minpoll 6 maxpoll 8
pool 2.ubuntu.pool.ntp.org iburst maxsources 2 minpoll 6 maxpoll 8
leapsectz right/UTC
minsources 3
log measurements statistics tracking selection
makestep 1.0 3
rtcsync
";
        assert_eq!(
            ClockConfig::from_chrony_conf(conf).unwrap(),
            ClockConfig::default()
        );
    }

    #[test]
    fn venue_server() {
        let cfg = ClockConfig::from_chrony_conf(
            "server 192.0.2.10 iburst minpoll 4 maxpoll 6\nminsources 1",
        )
        .unwrap();
        assert_eq!(cfg.sources[0].kind, SourceKind::Server);
        assert_eq!(cfg.sources[0].min_poll, 4);
        assert_eq!(cfg.min_sources, 1);
        assert!(ClockConfig::from_chrony_conf("minsources 2").is_err());
    }
}
