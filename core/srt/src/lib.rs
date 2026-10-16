//! SRT caller for TallyCam: sends MPEG-TS live to an SRT listener (Strom
//! receives with libsrt).
//!
//! The protocol is the pure-Rust `srt-protocol` crate (sans-IO); this crate
//! drives it with one thread that owns a UDP socket and waits in `poll(2)`.
//! No async runtime: one thread per connection is all a camera needs, and
//! it keeps the binary small and the timing predictable on iOS.
//!
//! - [`CallerConfig`]: host, port, streamid, latency, passphrase, pbkeylen.
//! - [`SrtSender`]: `start`, `send` (lock-free; one allocation per 64
//!   payloads), `stats`, `close`; events through [`SrtObserver`].
//! - Adaptive bitrate on the SRT thread: `start_adaptive_bitrate`,
//!   `set_adaptive_ceiling`, `stop_adaptive_bitrate`.

mod config;
mod queue;
mod sender;

pub use config::{CallerConfig, DEFAULT_LATENCY_MS, PAYLOAD_SIZE};
pub use sender::{
    BitrateCallback, SendError, SenderOptions, SrtEvent, SrtObserver, SrtSender, SrtStats,
    STATS_INTERVAL,
};
