//! Adaptive bitrate: lowers the video bitrate when the network cannot carry
//! it and raises it again, up to the operator's bitrate, when it can.
//!
//! The SRT caller (core/srt) runs it on its own thread, on each refresh of
//! the engine's statistics (every 250 ms), and reports the bitrate to the
//! app, which applies it to its encoder.
//!
//! The main signal is the SRT send buffer, as time: the span between the
//! oldest packet not yet acknowledged and the newest one queued. On a link
//! that keeps up it is about one RTT plus the ACK interval. When the link
//! carries less than the encoder makes, packets wait to go out and losses
//! wait for their retransmit, so it grows; once it nears the SRT latency,
//! packets are dropped as too late and the picture breaks up. The loop
//! cuts well before that. RTT growth (a queue in the network, before any
//! loss) and the loss rate the listener reports are early signs too.
//!
//! Latency comes first: a cut is large (to 60% when packets are dropped or
//! the buffer nears the latency), and the climb back is slow, after
//! a quiet period, more slowly near the bitrate where congestion last hit.
//!
//! Pure state machine; no allocation.

use super::MIN_KBPS;

/// One sample of the SRT caller's stats. Counters are totals since the
/// caller started (all connections); only their changes are used.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LinkSample {
    /// Monotonic time of the sample, ms.
    pub t_ms: u64,
    pub connected: bool,
    /// Smoothed RTT from the listener's ACKs, ms.
    pub rtt_ms: f64,
    /// SRT send buffer span (sent and not acknowledged, or not sent yet), ms.
    pub send_buffer_ms: f64,
    /// The TSBPD latency both sides agreed to, ms.
    pub latency_ms: u32,
    /// Unique data packets sent.
    pub packets_sent: u64,
    /// Packets the listener reported lost.
    pub lost_packets: u64,
    /// Packets dropped by the sender (too late, or its queue was full).
    pub dropped_packets: u64,
}

/// What the loop last saw, for the status and the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// At the operator's bitrate, or climbing back without trouble.
    Clear,
    /// Signs of a queue (RTT or send buffer growing, loss): cut gently.
    Congested,
    /// Packets dropped, or a queue that nears the latency: cut hard.
    Overloaded,
}

impl LinkState {
    /// The name in `status.stream.link`.
    pub fn as_str(self) -> &'static str {
        match self {
            LinkState::Clear => "clear",
            LinkState::Congested => "congested",
            LinkState::Overloaded => "overloaded",
        }
    }
}

/// Lower bound of the latency the thresholds scale with, ms.
const MIN_LATENCY_MS: f64 = 60.0;
/// Send buffer beyond RTT + ACK interval that counts as a queue: this share
/// of the latency, at least [`MIN_EXCESS_MS`].
const EXCESS_SHARE: f64 = 0.2;
const MIN_EXCESS_MS: f64 = 25.0;
/// ACK interval and slack in the send buffer of a link that keeps up, ms.
const ACK_SLACK_MS: f64 = 20.0;
/// RTT above the baseline that counts as a queue in the network: this
/// share of the latency, at least [`MIN_RTT_RISE_MS`].
const RTT_RISE_SHARE: f64 = 0.25;
const MIN_RTT_RISE_MS: f64 = 30.0;
/// Send buffer past this share of the latency with a queue, or twice the
/// queue that counts as congestion: overloaded.
const OVERLOAD_SHARE: f64 = 0.75;
/// Loss rate that counts as congestion.
const LOSS_RATE: f64 = 0.05;
/// The loss rate is taken over at least this many packets (half a second
/// at 1 Mbit/s), across samples at low bitrates.
const MIN_PACKETS_FOR_LOSS: u64 = 50;

/// Cut factor when overloaded and when congested.
const OVERLOAD_CUT: f64 = 0.6;
const CONGESTION_CUT: f64 = 0.8;
/// Congested samples in a row before a gentle cut (one can be a burst).
const CONGESTED_SAMPLES: u32 = 2;
/// Time between cuts, so the encoder's new rate shows in the buffer.
const CUT_SPACING_MS: u64 = 1000;
/// Quiet time after the last sign of trouble before climbing.
const HOLD_MS: u64 = 4000;
/// Time between steps up.
const STEP_SPACING_MS: u64 = 1000;
/// Step up: this share of the current bitrate, at least [`MIN_STEP_KBPS`];
/// [`NEAR_STEP`] within 10% of where congestion last hit.
const STEP: f64 = 0.06;
const NEAR_STEP: f64 = 0.02;
const MIN_STEP_KBPS: f64 = 100.0;
/// Bitrates go to the encoder in steps of this.
const ROUND_KBPS: u32 = 100;
/// The RTT baseline is the lowest RTT over two windows of this.
const RTT_WINDOW_MS: u64 = 15_000;

/// The loop. Create it with the operator's bitrate when the stream starts.
#[derive(Debug, Clone)]
pub struct AdaptiveBitrate {
    ceiling_kbps: u32,
    floor_kbps: u32,
    current_kbps: u32,
    state: LinkState,
    /// Previous sample's counters and time; None after a (re)connect.
    last: Option<LinkSample>,
    congested_samples: u32,
    last_cut_ms: Option<u64>,
    /// Last sample with any sign of trouble.
    last_trouble_ms: Option<u64>,
    last_step_ms: u64,
    /// The bitrate when congestion last hit; steps slow down near it.
    trouble_kbps: Option<u32>,
    rtt_min: RttMin,
    /// Packets sent and lost since the loss window opened.
    loss_sent: u64,
    loss_lost: u64,
    /// The last closed loss window was over [`LOSS_RATE`].
    lossy: bool,
}

impl AdaptiveBitrate {
    /// Starts at `ceiling_kbps`, the operator's bitrate.
    pub fn new(ceiling_kbps: u32) -> Self {
        let ceiling = ceiling_kbps.max(1);
        AdaptiveBitrate {
            ceiling_kbps: ceiling,
            floor_kbps: MIN_KBPS.min(ceiling),
            current_kbps: ceiling,
            state: LinkState::Clear,
            last: None,
            congested_samples: 0,
            last_cut_ms: None,
            last_trouble_ms: None,
            last_step_ms: 0,
            trouble_kbps: None,
            rtt_min: RttMin::default(),
            loss_sent: 0,
            loss_lost: 0,
            lossy: false,
        }
    }

    /// The bitrate the encoder should run, kbit/s.
    pub fn current_kbps(&self) -> u32 {
        self.current_kbps
    }

    pub fn ceiling_kbps(&self) -> u32 {
        self.ceiling_kbps
    }

    pub fn state(&self) -> LinkState {
        self.state
    }

    /// The operator changed the bitrate. Running at the old ceiling, the
    /// encoder follows at once; held below it, it never goes up here (the
    /// link carried only that much) and climbs to the new one as usual.
    /// Returns the bitrate to apply.
    pub fn set_ceiling(&mut self, kbps: u32) -> u32 {
        let at_ceiling = self.current_kbps >= self.ceiling_kbps;
        self.ceiling_kbps = kbps.max(1);
        self.floor_kbps = MIN_KBPS.min(self.ceiling_kbps);
        self.current_kbps = if at_ceiling {
            self.ceiling_kbps
        } else {
            self.current_kbps.min(self.ceiling_kbps)
        };
        self.current_kbps
    }

    /// Takes a sample; returns a new bitrate when it changed.
    pub fn update(&mut self, s: &LinkSample) -> Option<u32> {
        if !s.connected {
            // Reconnecting: the next connection may take another path.
            self.last = None;
            self.state = LinkState::Clear;
            self.congested_samples = 0;
            self.rtt_min = RttMin::default();
            self.loss_sent = 0;
            self.loss_lost = 0;
            self.lossy = false;
            return None;
        }
        let Some(last) = self.last.replace(*s) else {
            // First sample of a connection: counters only.
            return None;
        };
        if s.rtt_ms > 0.0 {
            self.rtt_min.add(s.t_ms, s.rtt_ms);
        }
        let state = self.assess(&last, s);
        self.state = state;
        if state != LinkState::Clear {
            self.last_trouble_ms = Some(s.t_ms);
        }
        let target = match state {
            LinkState::Overloaded => {
                self.congested_samples = 0;
                self.cut(s.t_ms, OVERLOAD_CUT)
            }
            LinkState::Congested => {
                self.congested_samples += 1;
                if self.congested_samples >= CONGESTED_SAMPLES {
                    self.cut(s.t_ms, CONGESTION_CUT)
                } else {
                    None
                }
            }
            LinkState::Clear => {
                self.congested_samples = 0;
                self.step_up(s.t_ms)
            }
        };
        let target = target?;
        if target == self.current_kbps {
            return None;
        }
        self.current_kbps = target;
        Some(target)
    }

    fn assess(&mut self, last: &LinkSample, s: &LinkSample) -> LinkState {
        self.loss_sent += s.packets_sent.saturating_sub(last.packets_sent);
        self.loss_lost += s.lost_packets.saturating_sub(last.lost_packets);
        if self.loss_sent >= MIN_PACKETS_FOR_LOSS {
            self.lossy = self.loss_lost as f64 > self.loss_sent as f64 * LOSS_RATE;
            self.loss_sent = 0;
            self.loss_lost = 0;
        }
        if s.dropped_packets > last.dropped_packets {
            return LinkState::Overloaded;
        }
        if s.rtt_ms <= 0.0 {
            // No ACK yet: nothing to measure the buffer against.
            return LinkState::Clear;
        }
        let latency = f64::from(s.latency_ms).max(MIN_LATENCY_MS);
        // Includes this sample's RTT, so never above it.
        let base_rtt = self.rtt_min.get().unwrap_or(s.rtt_ms);
        // The send buffer of a link that keeps up, and the queue beyond it.
        let excess = s.send_buffer_ms - s.rtt_ms - ACK_SLACK_MS;
        let queue = (latency * EXCESS_SHARE).max(MIN_EXCESS_MS);
        if excess > 2.0 * queue
            // Near the latency with a queue (a long RTT alone is no reason:
            // a lower bitrate does not shorten it).
            || (s.send_buffer_ms > latency * OVERLOAD_SHARE && excess > queue / 2.0)
        {
            return LinkState::Overloaded;
        }
        let rtt_rise = s.rtt_ms - base_rtt;
        if excess > queue
            || rtt_rise > (latency * RTT_RISE_SHARE).max(MIN_RTT_RISE_MS)
            || self.lossy
        {
            LinkState::Congested
        } else {
            LinkState::Clear
        }
    }

    fn cut(&mut self, t_ms: u64, factor: f64) -> Option<u32> {
        if self
            .last_cut_ms
            .is_some_and(|t| t_ms.saturating_sub(t) < CUT_SPACING_MS)
        {
            return None;
        }
        self.last_cut_ms = Some(t_ms);
        self.congested_samples = 0;
        self.trouble_kbps = Some(self.current_kbps);
        Some(self.round(f64::from(self.current_kbps) * factor))
    }

    fn step_up(&mut self, t_ms: u64) -> Option<u32> {
        if self.current_kbps >= self.ceiling_kbps {
            return None;
        }
        let quiet = self
            .last_trouble_ms
            .is_none_or(|t| t_ms.saturating_sub(t) >= HOLD_MS);
        if !quiet || t_ms.saturating_sub(self.last_step_ms) < STEP_SPACING_MS {
            return None;
        }
        self.last_step_ms = t_ms;
        let current = f64::from(self.current_kbps);
        let near = self
            .trouble_kbps
            .is_some_and(|k| current >= f64::from(k) * 0.9);
        let step = (current * if near { NEAR_STEP } else { STEP }).max(MIN_STEP_KBPS);
        // Past where congestion hit without trouble: forget it.
        if self.trouble_kbps.is_some_and(|k| current > f64::from(k)) {
            self.trouble_kbps = None;
        }
        Some(self.round(current + step))
    }

    fn round(&self, kbps: f64) -> u32 {
        let k = ((kbps / f64::from(ROUND_KBPS)).round() as u32) * ROUND_KBPS;
        k.clamp(self.floor_kbps, self.ceiling_kbps)
    }
}

/// Lowest RTT over the current and the previous window: follows a path
/// whose RTT went up for good within two windows.
#[derive(Debug, Clone, Copy, Default)]
struct RttMin {
    window_start_ms: u64,
    current: Option<f64>,
    previous: Option<f64>,
}

impl RttMin {
    fn add(&mut self, t_ms: u64, rtt_ms: f64) {
        if self.current.is_none() && self.previous.is_none() {
            self.window_start_ms = t_ms;
        } else if t_ms.saturating_sub(self.window_start_ms) >= RTT_WINDOW_MS {
            self.previous = self.current;
            self.current = None;
            self.window_start_ms = t_ms;
        }
        self.current = Some(self.current.map_or(rtt_ms, |m| m.min(rtt_ms)));
    }

    fn get(&self) -> Option<f64> {
        match (self.current, self.previous) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TICK: u64 = 250;
    /// Payloads per second at 6 Mbit/s.
    const PPS: u64 = 6_000_000 / 8 / 1316;

    /// A link simulation: the encoder's bitrate against a capacity; what does
    /// not fit waits in the send buffer.
    struct Link {
        t_ms: u64,
        sent: u64,
        lost: u64,
        dropped: u64,
        backlog_ms: f64,
        base_rtt: f64,
        latency_ms: u32,
    }

    impl Link {
        fn new() -> Self {
            Link {
                t_ms: 0,
                sent: 0,
                lost: 0,
                dropped: 0,
                backlog_ms: 0.0,
                base_rtt: 20.0,
                latency_ms: 120,
            }
        }

        /// One tick at `kbps` over a link of `capacity_kbps`.
        fn tick(&mut self, kbps: u32, capacity_kbps: u32) -> LinkSample {
            self.t_ms += TICK;
            self.sent += PPS * TICK / 1000;
            // The backlog grows by the excess, drains by the spare capacity.
            let ratio = f64::from(kbps) / f64::from(capacity_kbps);
            self.backlog_ms = (self.backlog_ms + (ratio - 1.0) * TICK as f64).max(0.0);
            if self.backlog_ms > f64::from(self.latency_ms) {
                self.dropped += 10;
                self.backlog_ms = f64::from(self.latency_ms);
            }
            let rtt = self.base_rtt + self.backlog_ms * 0.5;
            LinkSample {
                t_ms: self.t_ms,
                connected: true,
                rtt_ms: rtt,
                send_buffer_ms: rtt + 10.0 + self.backlog_ms,
                latency_ms: self.latency_ms,
                packets_sent: self.sent,
                lost_packets: self.lost,
                dropped_packets: self.dropped,
            }
        }
    }

    fn run(abr: &mut AdaptiveBitrate, link: &mut Link, capacity: u32, seconds: u64) {
        for _ in 0..seconds * 1000 / TICK {
            let s = link.tick(abr.current_kbps(), capacity);
            abr.update(&s);
        }
    }

    #[test]
    fn stays_at_the_ceiling_on_a_good_link() {
        let mut abr = AdaptiveBitrate::new(6000);
        let mut link = Link::new();
        run(&mut abr, &mut link, 20_000, 30);
        assert_eq!(abr.current_kbps(), 6000);
        assert_eq!(abr.state(), LinkState::Clear);
        assert_eq!(link.dropped, 0);
    }

    #[test]
    fn falls_below_a_narrow_link_without_drops_and_climbs_back() {
        let mut abr = AdaptiveBitrate::new(6000);
        let mut link = Link::new();
        run(&mut abr, &mut link, 20_000, 5);
        // The link narrows to 4.5 Mbit/s.
        run(&mut abr, &mut link, 4500, 30);
        assert!(abr.current_kbps() <= 4600, "{}", abr.current_kbps());
        assert!(abr.current_kbps() >= 2500, "{}", abr.current_kbps());
        assert_eq!(link.dropped, 0, "cut before the buffer reaches the latency");
        // It widens again: back at the ceiling within a minute.
        run(&mut abr, &mut link, 20_000, 60);
        assert_eq!(abr.current_kbps(), 6000);
    }

    #[test]
    fn cuts_hard_on_drops_and_respects_the_floor() {
        let mut abr = AdaptiveBitrate::new(6000);
        let mut link = Link::new();
        run(&mut abr, &mut link, 20_000, 2);
        let mut s = link.tick(6000, 20_000);
        s.dropped_packets += 5;
        assert_eq!(abr.update(&s), Some(3600));
        assert_eq!(abr.state(), LinkState::Overloaded);
        // A link narrower than the floor: it stops at the floor.
        run(&mut abr, &mut link, 300, 30);
        assert_eq!(abr.current_kbps(), MIN_KBPS);
    }

    #[test]
    fn one_slow_sample_is_not_enough() {
        let mut abr = AdaptiveBitrate::new(6000);
        let mut link = Link::new();
        run(&mut abr, &mut link, 20_000, 2);
        let mut s = link.tick(6000, 20_000);
        s.send_buffer_ms = s.rtt_ms + 50.0;
        assert_eq!(abr.update(&s), None);
        assert_eq!(abr.state(), LinkState::Congested);
        let s = link.tick(6000, 20_000);
        assert_eq!(abr.update(&s), None);
        assert_eq!(abr.current_kbps(), 6000);
        // Two in a row: a gentle cut.
        let mut s = link.tick(6000, 20_000);
        s.send_buffer_ms = s.rtt_ms + 50.0;
        assert_eq!(abr.update(&s), None);
        let mut s = link.tick(6000, 20_000);
        s.send_buffer_ms = s.rtt_ms + 50.0;
        assert_eq!(abr.update(&s), Some(4800));
    }

    #[test]
    fn a_long_rtt_is_not_congestion() {
        // 5G with 70 ms RTT and 120 ms latency: the buffer of a link that
        // keeps up is already past half the latency.
        let mut abr = AdaptiveBitrate::new(6000);
        let mut link = Link::new();
        link.base_rtt = 70.0;
        run(&mut abr, &mut link, 20_000, 10);
        assert_eq!(abr.current_kbps(), 6000);
        let mut s = link.tick(6000, 20_000);
        s.send_buffer_ms = s.rtt_ms + 30.0;
        assert_eq!(abr.update(&s), None);
        assert_eq!(abr.state(), LinkState::Clear);
    }

    #[test]
    fn heavy_loss_is_congestion() {
        let mut abr = AdaptiveBitrate::new(6000);
        let mut link = Link::new();
        run(&mut abr, &mut link, 20_000, 2);
        let mut cut = None;
        for _ in 0..4 {
            link.lost += PPS * TICK / 1000 / 5;
            let s = link.tick(6000, 20_000);
            cut = cut.or(abr.update(&s));
        }
        assert_eq!(cut, Some(4800));
    }

    #[test]
    fn operator_changes_and_reconnects() {
        let mut abr = AdaptiveBitrate::new(6000);
        let mut link = Link::new();
        run(&mut abr, &mut link, 20_000, 2);
        // At the ceiling, the encoder follows it both ways.
        assert_eq!(abr.set_ceiling(4000), 4000);
        assert_eq!(abr.set_ceiling(8000), 8000);
        // Held down: a higher ceiling waits for the climb, and a lower one
        // above the current bitrate does not raise it.
        let mut s = link.tick(8000, 20_000);
        s.dropped_packets += 1;
        assert_eq!(abr.update(&s), Some(4800));
        assert_eq!(abr.state(), LinkState::Overloaded);
        assert_eq!(abr.set_ceiling(10_000), 4800);
        assert_eq!(abr.set_ceiling(6000), 4800);
        assert_eq!(abr.set_ceiling(3000), 3000);
        // Disconnected samples change nothing but the state; the first one
        // after a reconnect only sets the counters.
        let mut s = link.tick(3000, 20_000);
        s.connected = false;
        assert_eq!(abr.update(&s), None);
        assert_eq!(abr.state(), LinkState::Clear, "a new path starts clear");
        let mut s = link.tick(3000, 20_000);
        s.dropped_packets += 100;
        assert_eq!(abr.update(&s), None);
        assert_eq!(abr.current_kbps(), 3000);
    }

    #[test]
    fn loss_counts_at_low_bitrates() {
        // At 1.5 Mbit/s a sample holds about 36 packets: the loss rate is
        // taken over several samples.
        let mut abr = AdaptiveBitrate::new(1500);
        let mut link = Link::new();
        run(&mut abr, &mut link, 20_000, 2);
        let per_tick = PPS * TICK / 1000;
        let mut cut = None;
        for _ in 0..8 {
            link.sent += 36;
            link.sent -= per_tick;
            link.lost += 6;
            let s = link.tick(1500, 20_000);
            cut = cut.or(abr.update(&s));
        }
        assert_eq!(cut, Some(1200));
    }

    #[test]
    fn rtt_baseline_follows_a_new_path() {
        let mut m = RttMin::default();
        m.add(0, 20.0);
        m.add(1000, 80.0);
        assert_eq!(m.get(), Some(20.0));
        m.add(RTT_WINDOW_MS, 80.0);
        assert_eq!(m.get(), Some(20.0));
        m.add(2 * RTT_WINDOW_MS, 80.0);
        assert_eq!(m.get(), Some(80.0));
    }
}
