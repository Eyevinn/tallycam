//! TallyCam shared core.
//!
//! - [`clock`]: host clock to TAI/UTC mapping; a chrony-like NTP discipline
//!   as a pure state machine, or the system clock on hosts that run chrony.
//! - [`sei`]: the LCP capture timestamp SEI (H.264 and HEVC).
//! - [`protocol`]: LCP device protocol message types.
//! - [`ts`]: the MPEG-TS muxer (H.264/HEVC with the timestamp SEI, Opus
//!   or AAC), sans-IO and allocation free per frame.
//! - [`genlock`]: soft genlock, a PI loop that locks frame timing to the TAI
//!   frame grid.
//!
//! The pure parts have no platform dependencies and build for
//! `wasm32-unknown-unknown`; the `driver` feature adds a std driver (UDP
//! sockets, DNS, timer thread) for macOS, iOS and Linux.

/// Default and maximum video bitrate per format and codec.
pub mod bitrate;
pub mod clock;
/// Device keys: ids from public keys, the signed `auth` handshake.
pub mod device_key;
/// Auto ISO and flicker-free shutters.
pub mod exposure;
pub mod genlock;
pub mod protocol;
pub mod sei;
pub mod ts;
