//! The SRT caller: UniFFI for setup, events and stats; the C ABI
//! `lc_srt_send` for the per-frame send.

use std::sync::Arc;

use crate::host;

#[derive(Debug, Clone, uniffi::Record)]
pub struct SrtCallerOptions {
    pub host: String,
    pub port: u16,
    pub stream_id: Option<String>,
    /// SRTO_LATENCY, ms; SRT plays out with the larger of both sides'.
    pub latency_ms: u16,
    /// 10-79 characters; `None` = no encryption.
    pub passphrase: Option<String>,
    /// 16, 24 or 32 (AES-128/192/256); 0 = 16 with a passphrase.
    pub pbkeylen: u8,
    pub connect_timeout_ms: u32,
    /// Connection lost after this long without a packet from the listener.
    pub peer_idle_timeout_ms: u32,
    /// Pacing ceiling, bits per second; 0 = 1 Gbit/s.
    pub max_bitrate_bps: u64,
    /// Connect again after a loss or failed attempt instead of stopping.
    pub reconnect: bool,
}

impl From<tallycam_srt::CallerConfig> for SrtCallerOptions {
    fn from(c: tallycam_srt::CallerConfig) -> Self {
        SrtCallerOptions {
            host: c.host,
            port: c.port,
            stream_id: c.stream_id,
            latency_ms: c.latency_ms,
            passphrase: c.passphrase,
            pbkeylen: c.pbkeylen,
            connect_timeout_ms: c.connect_timeout.as_millis() as u32,
            peer_idle_timeout_ms: c.peer_idle_timeout.as_millis() as u32,
            max_bitrate_bps: c.max_bandwidth * 8,
            reconnect: c.reconnect,
        }
    }
}

impl From<SrtCallerOptions> for tallycam_srt::CallerConfig {
    fn from(o: SrtCallerOptions) -> Self {
        tallycam_srt::CallerConfig {
            host: o.host,
            port: o.port,
            stream_id: o.stream_id,
            latency_ms: o.latency_ms,
            passphrase: o.passphrase,
            pbkeylen: o.pbkeylen,
            connect_timeout: std::time::Duration::from_millis(o.connect_timeout_ms.into()),
            peer_idle_timeout: std::time::Duration::from_millis(o.peer_idle_timeout_ms.into()),
            max_bandwidth: o.max_bitrate_bps / 8,
            reconnect: o.reconnect,
        }
    }
}

#[derive(Debug, uniffi::Error)]
pub enum SrtError {
    Config { message: String },
}

impl std::fmt::Display for SrtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SrtError::Config { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SrtError {}

/// Parses `srt://host:port?streamid=&latency=&passphrase=&pbkeylen=` into
/// caller options with the defaults (timeouts 5 s, 50 Mbit/s pacing, no
/// reconnect).
#[uniffi::export]
pub fn srt_caller_options_from_url(url: String) -> Result<SrtCallerOptions, SrtError> {
    tallycam_srt::CallerConfig::from_url(&url)
        .map(Into::into)
        .map_err(|message| SrtError::Config { message })
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SrtEvent {
    Connecting {
        attempt: u32,
    },
    /// `latency_ms`: what both sides agreed to (TSBPD).
    Connected {
        latency_ms: u32,
        peer: String,
    },
    /// `reconnecting`: another attempt follows (options.reconnect).
    Disconnected {
        reason: String,
        reconnecting: bool,
    },
}

/// Implemented in Swift. Called on the SRT thread: hop to your own queue.
#[uniffi::export(with_foreign)]
pub trait SrtListener: Send + Sync {
    fn on_event(&self, event: SrtEvent);
    /// The first (`last` false) or last payload of a video frame went out
    /// for the first time. Host ns (lc_host_now_ns clock).
    fn on_frame_sent(&self, capture_ns: i64, enqueued_ns: i64, sent_ns: i64, last: bool);
}

struct ListenerBridge(Arc<dyn SrtListener>);

impl tallycam_srt::SrtObserver for ListenerBridge {
    fn on_event(&self, event: tallycam_srt::SrtEvent) {
        self.0.on_event(match event {
            tallycam_srt::SrtEvent::Connecting { attempt } => SrtEvent::Connecting { attempt },
            tallycam_srt::SrtEvent::Connected { latency_ms, peer } => {
                SrtEvent::Connected { latency_ms, peer }
            }
            tallycam_srt::SrtEvent::Disconnected {
                reason,
                reconnecting,
            } => SrtEvent::Disconnected {
                reason,
                reconnecting,
            },
        });
    }

    fn on_frame_sent(&self, capture_ns: i64, enqueued_ns: i64, sent_ns: i64, last: bool) {
        self.0.on_frame_sent(capture_ns, enqueued_ns, sent_ns, last);
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SrtStats {
    pub connected: bool,
    pub rtt_ms: f64,
    pub retransmitted_packets: u64,
    pub received_naks: u64,
    /// Packets the listener reported lost.
    pub lost_packets: u64,
    /// Dropped by the sender (too late, or the send queue was full).
    pub dropped_packets: u64,
    pub packets_in_flight: u64,
    /// Span of the send buffer (not acknowledged, or not sent yet), ms;
    /// the main signal of adaptive bitrate.
    pub send_buffer_ms: f64,
    /// 0 before the first connection.
    pub negotiated_latency_ms: u32,
    /// With IP/UDP headers, all connections.
    pub bytes_sent: u64,
    pub packets_sent: u64,
    pub connections: u32,
    /// Adaptive bitrate's bitrate now, kbit/s; 0 while it is off.
    pub target_kbps: u32,
    /// What adaptive bitrate last saw (`clear | congested | overloaded`, as
    /// in `status.stream.link`); None while it is off.
    pub link: Option<String>,
}

/// Implemented in Swift. Called on the SRT thread: hop to your own queue.
#[uniffi::export(with_foreign)]
pub trait BitrateListener: Send + Sync {
    /// Adaptive bitrate's bitrate for the encoder, kbit/s.
    fn on_bitrate(&self, kbps: u32);
}

#[derive(uniffi::Object)]
pub struct SrtCaller {
    sender: Arc<tallycam_srt::SrtSender>,
}

#[uniffi::export]
impl SrtCaller {
    /// Starts the SRT thread, which connects (events on `listener`).
    #[uniffi::constructor]
    pub fn start(
        options: SrtCallerOptions,
        listener: Arc<dyn SrtListener>,
    ) -> Result<Arc<Self>, SrtError> {
        let sender = tallycam_srt::SrtSender::start(
            options.into(),
            tallycam_srt::SenderOptions {
                host_now_ns: Arc::new(|| host().now_ns()),
            },
            Arc::new(ListenerBridge(listener)),
        )
        .map_err(|message| SrtError::Config { message })?;
        Ok(Arc::new(SrtCaller {
            sender: Arc::new(sender),
        }))
    }

    /// For the C hot path (`lc_srt_send`): valid while this object lives.
    pub fn hot_handle(&self) -> u64 {
        Arc::as_ptr(&self.sender) as u64
    }

    /// Convenience send through UniFFI (copies `data` into a RustBuffer);
    /// the per-frame path is `lc_srt_send`. Returns `lc_srt_send`'s code.
    pub fn send(&self, data: Vec<u8>, frame_capture_ns: i64) -> i32 {
        send_code(self.sender.send(&data, frame_capture_ns, 0))
    }

    pub fn is_connected(&self) -> bool {
        self.sender.is_connected()
    }

    pub fn stats(&self) -> SrtStats {
        let s = self.sender.stats();
        SrtStats {
            connected: s.connected,
            rtt_ms: s.rtt_ms,
            retransmitted_packets: s.retransmitted_packets,
            received_naks: s.received_naks,
            lost_packets: s.lost_packets,
            dropped_packets: s.dropped_packets,
            packets_in_flight: s.packets_in_flight,
            send_buffer_ms: s.send_buffer_ms,
            negotiated_latency_ms: s.negotiated_latency_ms,
            bytes_sent: s.bytes_sent,
            packets_sent: s.packets_sent,
            connections: s.connections,
            target_kbps: s.target_kbps,
            link: s.link.map(|l| l.as_str().to_string()),
        }
    }

    /// Adaptive bitrate on the SRT thread, from `ceiling_kbps` (the
    /// operator's bitrate); `listener` hears each new bitrate, and the
    /// first one at once. Again: starts afresh.
    pub fn start_adaptive_bitrate(&self, ceiling_kbps: u32, listener: Arc<dyn BitrateListener>) {
        self.sender.start_adaptive_bitrate(
            ceiling_kbps,
            Arc::new(move |kbps| listener.on_bitrate(kbps)),
        );
    }

    /// The operator's bitrate changed; ignored while adaptive bitrate is off.
    pub fn set_adaptive_ceiling(&self, ceiling_kbps: u32) {
        self.sender.set_adaptive_ceiling(ceiling_kbps);
    }

    pub fn stop_adaptive_bitrate(&self) {
        self.sender.stop_adaptive_bitrate();
    }

    /// Sends SRT SHUTDOWN, stops and joins the thread. No events after it
    /// returns. Idempotent; dropping the object does the same.
    pub fn close(&self) {
        self.sender.close();
    }
}

pub(crate) fn send_code(r: Result<usize, tallycam_srt::SendError>) -> i32 {
    match r {
        Ok(n) => n as i32,
        Err(tallycam_srt::SendError::NotConnected) => -1,
        Err(tallycam_srt::SendError::QueueFull) => -2,
        Err(tallycam_srt::SendError::Busy) => -3,
    }
}

/// # Safety
/// `handle` comes from `SrtCaller.hot_handle()` of a live object; `data`
/// points to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn lc_srt_send(
    handle: u64,
    data: *const u8,
    len: usize,
    frame_capture_ns: i64,
    now_ns: i64,
) -> i32 {
    if handle == 0 || data.is_null() || len == 0 {
        return -4;
    }
    // SAFETY: the caller guarantees the handle's object is alive and the
    // buffer readable.
    let (sender, data) = unsafe {
        (
            &*(handle as *const tallycam_srt::SrtSender),
            std::slice::from_raw_parts(data, len),
        )
    };
    send_code(sender.send(data, frame_capture_ns, now_ns))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoListener;

    impl SrtListener for NoListener {
        fn on_event(&self, _event: SrtEvent) {}
        fn on_frame_sent(&self, _capture_ns: i64, _enqueued_ns: i64, _sent_ns: i64, _last: bool) {}
    }

    #[test]
    fn srt_hot_path_codes_and_close() {
        // Nobody answers on the discard port: the caller stays connecting.
        let mut options =
            srt_caller_options_from_url("srt://127.0.0.1:9?latency=0".into()).unwrap();
        options.connect_timeout_ms = 60_000;
        let caller = SrtCaller::start(options, Arc::new(NoListener)).unwrap();
        let ts = [0x47u8; 188 * 7];
        let h = caller.hot_handle();
        assert_eq!(unsafe { lc_srt_send(h, ts.as_ptr(), ts.len(), 0, 0) }, -1);
        assert_eq!(unsafe { lc_srt_send(0, ts.as_ptr(), ts.len(), 0, 0) }, -4);
        assert_eq!(unsafe { lc_srt_send(h, ts.as_ptr(), 0, 0, 0) }, -4);
        assert!(!caller.stats().connected);
        let t = std::time::Instant::now();
        caller.close();
        caller.close();
        assert!(t.elapsed() < std::time::Duration::from_millis(500));
        assert!(
            srt_caller_options_from_url("srt://example.com:1?passphrase=short".into()).is_err()
        );
    }
}
