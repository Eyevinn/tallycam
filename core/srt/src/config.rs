//! Caller configuration and the `srt://` URL syntax the app accepts.

use std::time::Duration;

/// Payload of one SRT data packet: 7 TS packets, the de facto standard for
/// MPEG-TS over SRT.
pub const PAYLOAD_SIZE: usize = 7 * 188;

/// Default latency (ms) when the URL has none; the app's default.
pub const DEFAULT_LATENCY_MS: u16 = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerConfig {
    pub host: String,
    pub port: u16,
    pub stream_id: Option<String>,
    /// Receiver and sender latency in ms (SRTO_LATENCY). SRT plays out with
    /// the larger of ours and the listener's.
    pub latency_ms: u16,
    /// 10 to 79 characters; `None` = no encryption.
    pub passphrase: Option<String>,
    /// AES key length in bytes: 16, 24 or 32; 0 = 16 when a passphrase is set.
    pub pbkeylen: u8,
    /// Give up a handshake after this long.
    pub connect_timeout: Duration,
    /// Declare the connection lost after this long without a packet from the
    /// listener (SRTO_PEERIDLETIMEO).
    pub peer_idle_timeout: Duration,
    /// Pacing ceiling, bytes per second (SRTO_MAXBW). 0 = 1 Gbit/s.
    pub max_bandwidth: u64,
    /// After a lost connection or a failed handshake, connect again (with a
    /// fresh socket and DNS lookup) instead of stopping.
    pub reconnect: bool,
}

impl CallerConfig {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        CallerConfig {
            host: host.into(),
            port,
            stream_id: None,
            latency_ms: DEFAULT_LATENCY_MS,
            passphrase: None,
            pbkeylen: 0,
            connect_timeout: Duration::from_secs(5),
            peer_idle_timeout: Duration::from_secs(5),
            // The Swift sender's pace: a keyframe leaves in a few ms instead
            // of tens of ms, which keeps the latency budget for retransmits.
            max_bandwidth: 50_000_000 / 8,
            reconnect: false,
        }
    }

    /// Checks the values SRT itself restricts.
    pub fn validate(&self) -> Result<(), String> {
        if self.host.is_empty() {
            return Err("URL has no host".into());
        }
        if self.port == 0 {
            return Err("URL has no port".into());
        }
        if let Some(p) = &self.passphrase {
            if !(10..=79).contains(&p.len()) {
                return Err("passphrase must be 10-79 characters".into());
            }
        }
        if !matches!(self.pbkeylen, 0 | 16 | 24 | 32) {
            return Err("pbkeylen must be 16, 24 or 32".into());
        }
        if let Some(id) = &self.stream_id {
            if id.len() > 512 {
                return Err("streamid is longer than 512 bytes".into());
            }
        }
        Ok(())
    }

    /// Parses `srt://host:port?streamid=..&latency=..&passphrase=..&pbkeylen=..`
    /// (caller mode only). Unknown parameters are ignored.
    pub fn from_url(url: &str) -> Result<Self, String> {
        let url = url.trim();
        let rest = match url.get(..6) {
            Some(s) if s.eq_ignore_ascii_case("srt://") => &url[6..],
            _ => return Err("URL must start with srt://".into()),
        };
        let (authority, query) = match rest.split_once('?') {
            Some((a, q)) => (a, q),
            None => (rest, ""),
        };
        let authority = authority.trim_end_matches('/');
        let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
            let (h, p) = v6.split_once("]:").ok_or("URL has no port")?;
            (h, p)
        } else {
            authority.rsplit_once(':').ok_or("URL has no port")?
        };
        let port: u16 = port.parse().map_err(|_| "URL has no port".to_string())?;
        let mut cfg = CallerConfig::new(host, port);
        for item in query.split('&').filter(|s| !s.is_empty()) {
            let (key, value) = item.split_once('=').unwrap_or((item, ""));
            let value = percent_decode(value)?;
            match key.to_ascii_lowercase().as_str() {
                "streamid" => cfg.stream_id = (!value.is_empty()).then_some(value),
                "latency" => {
                    cfg.latency_ms = value
                        .parse()
                        .map_err(|_| "latency must be 0-65535 ms".to_string())?
                }
                "passphrase" => cfg.passphrase = (!value.is_empty()).then_some(value),
                "pbkeylen" => {
                    cfg.pbkeylen = value
                        .parse()
                        .map_err(|_| "pbkeylen must be 16, 24 or 32".to_string())?
                }
                "mode" if !value.eq_ignore_ascii_case("caller") => {
                    return Err("Only caller mode is supported".into())
                }
                _ => {}
            }
        }
        cfg.validate()?;
        Ok(cfg)
    }
}

fn percent_decode(s: &str) -> Result<String, String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hex = s.get(i + 1..i + 3).ok_or("bad percent escape in URL")?;
                out.push(u8::from_str_radix(hex, 16).map_err(|_| "bad percent escape in URL")?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| "URL parameter is not UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_caller_urls() {
        let c = CallerConfig::from_url(
            "srt://192.0.2.1:9100?streamid=%23!%3A%3Ar%3Dcam1&latency=0&passphrase=0123456789&pbkeylen=32",
        )
        .unwrap();
        assert_eq!(c.host, "192.0.2.1");
        assert_eq!(c.port, 9100);
        assert_eq!(c.stream_id.as_deref(), Some("#!::r=cam1"));
        assert_eq!(c.latency_ms, 0);
        assert_eq!(c.passphrase.as_deref(), Some("0123456789"));
        assert_eq!(c.pbkeylen, 32);

        let c = CallerConfig::from_url("SRT://[2001:db8::1]:7001").unwrap();
        assert_eq!(c.host, "2001:db8::1");
        assert_eq!(c.latency_ms, DEFAULT_LATENCY_MS);
        assert!(c.passphrase.is_none());

        assert!(CallerConfig::from_url("udp://example.com:1").is_err());
        assert!(CallerConfig::from_url("srt://example.com").is_err());
        assert!(CallerConfig::from_url("srt://example.com:1?passphrase=short").is_err());
        assert!(CallerConfig::from_url("srt://example.com:1?pbkeylen=20").is_err());
        assert!(CallerConfig::from_url("srt://example.com:1?mode=listener").is_err());
        assert!(CallerConfig::from_url("srt://example.com:1?mode=caller").is_ok());
        assert!(CallerConfig::from_url("srt://example.com:1?x=%4").is_err());
    }
}
