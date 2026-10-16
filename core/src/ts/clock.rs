/// The MPEG-TS clock of the stream: PTS and PCR come straight from the
/// capture host clock (ns, the clock of the camera's and microphone's
/// sample times), in 90 kHz units modulo 2^33.
///
/// A sample's PTS is its capture time. The PCR says what time it is on that
/// clock when the packets leave the muxer, held back by `pcr_delay_ns`:
/// `PCR = now - pcr_delay_ns`. A receiver buffers each sample for
/// `PTS - PCR = pcr_delay_ns - (now - capture)`, its time to spare. So the
/// delay must cover the slowest stream's capture-to-mux time, and anything
/// above that is latency a syncing receiver adds for nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TsClock {
    /// How far the PCR runs behind real time, ns.
    pub pcr_delay_ns: i64,
}

impl TsClock {
    pub const WRAP: i64 = 1 << 33;

    pub fn new(pcr_delay_ns: i64) -> Self {
        TsClock { pcr_delay_ns }
    }

    /// Host ns to 33-bit 90 kHz ticks, rounded towards zero.
    pub fn ticks(host_ns: i64) -> u64 {
        let ticks = host_ns / 100_000 * 9 + (host_ns % 100_000) * 9 / 100_000;
        (ticks & (Self::WRAP - 1)) as u64
    }

    /// PTS of a sample captured at `host_ns`.
    pub fn pts(&self, host_ns: i64) -> u64 {
        Self::ticks(host_ns)
    }

    /// PCR base for packets leaving the muxer at `now_ns`.
    pub fn pcr(&self, now_ns: i64) -> u64 {
        Self::ticks(now_ns - self.pcr_delay_ns)
    }

    /// What a receiver has to spare for a sample captured at `capture_ns`
    /// and muxed at `now_ns`, ns: `PTS - PCR`. Negative means late.
    pub fn slack_ns(&self, capture_ns: i64, now_ns: i64) -> i64 {
        self.pcr_delay_ns - (now_ns - capture_ns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;

    #[test]
    fn ticks_are_host_time_in_90_khz() {
        assert_eq!(TsClock::ticks(0), 0);
        assert_eq!(TsClock::ticks(1_000_000_000), 90_000);
        assert_eq!(TsClock::ticks(11_111), 0, "rounds down");
        assert_eq!(TsClock::ticks(11_112), 1);
        // 2^33 ticks is 95443.7 s: the clock wraps like any MPEG-TS clock.
        let wrap_ns = 95_443_717_688_889;
        assert_eq!(TsClock::ticks(wrap_ns), 0);
        assert_eq!(TsClock::ticks(wrap_ns + 1_000_000_000), 90_000);
    }

    #[test]
    fn slack_is_delay_minus_time_on_the_phone() {
        let clock = TsClock::new(90 * MS);
        let capture = 10_000 * MS;
        let now = capture + 30 * MS;
        assert_eq!(clock.pts(capture) - clock.pcr(now), 60 * 90);
        assert_eq!(clock.slack_ns(capture, now), 60 * MS);
        assert!(clock.slack_ns(capture, capture + 100 * MS) < 0, "late");
    }
}
