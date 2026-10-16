//! NTPv4 packet codec (RFC 5905), client mode only.

/// Seconds from the NTP epoch (1900-01-01) to the Unix epoch (1970-01-01).
pub const NTP_UNIX_OFFSET_S: i64 = 2_208_988_800;
pub const HEADER_LEN: usize = 48;
pub const PORT: u16 = 123;
pub const VERSION: u8 = 4;
pub const MODE_CLIENT: u8 = 3;
pub const MODE_SERVER: u8 = 4;
/// Leap indicator 3: the server clock is not synchronized (also used by KoD).
pub const LEAP_UNSYNC: u8 = 3;
pub const MAX_STRATUM: u8 = 16;

/// A 64-bit NTP timestamp: seconds since 1900 in the high 32 bits (modulo
/// the 136-year era), binary fraction in the low 32 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct NtpTimestamp(pub u64);

impl NtpTimestamp {
    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub fn from_unix_ns(ns: i64) -> Self {
        let secs = ns.div_euclid(1_000_000_000);
        let nanos = ns.rem_euclid(1_000_000_000) as u64;
        let ntp_secs = (secs + NTP_UNIX_OFFSET_S) as u64 & 0xFFFF_FFFF;
        let frac = ((nanos << 32) + 500_000_000) / 1_000_000_000;
        NtpTimestamp((ntp_secs << 32) + frac.min(0xFFFF_FFFF))
    }

    /// Unix ns, choosing the 136-year era that puts the result closest to
    /// `pivot_unix_ns` (our best idea of the current time). Correct as long
    /// as the pivot is within 68 years of the truth, also across the 2036
    /// era rollover.
    pub fn to_unix_ns(self, pivot_unix_ns: i64) -> i64 {
        let secs = (self.0 >> 32) as i64;
        let frac = self.0 & 0xFFFF_FFFF;
        let nanos = ((frac * 1_000_000_000) + (1 << 31)) >> 32;
        let pivot_ntp = pivot_unix_ns.div_euclid(1_000_000_000) + NTP_UNIX_OFFSET_S;
        let era = (pivot_ntp - secs + (1 << 31)).div_euclid(1 << 32);
        (era * (1 << 32) + secs - NTP_UNIX_OFFSET_S) * 1_000_000_000 + nanos as i64
    }
}

/// NTP short format (16.16 seconds) to seconds.
pub fn short_to_secs(v: u32) -> f64 {
    v as f64 / 65536.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NtpPacket {
    pub leap: u8,
    pub version: u8,
    pub mode: u8,
    pub stratum: u8,
    pub poll: i8,
    pub precision: i8,
    pub root_delay: u32,
    pub root_dispersion: u32,
    pub reference_id: u32,
    pub reference_ts: NtpTimestamp,
    pub origin_ts: NtpTimestamp,
    pub receive_ts: NtpTimestamp,
    pub transmit_ts: NtpTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketError {
    TooShort,
    BadVersion(u8),
}

/// Kiss-o'-Death codes a client must honour (RFC 5905, 7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KissCode {
    /// Rate exceeded: back off.
    Rate,
    /// Access denied: stop using the server.
    Deny,
    /// Access restricted: stop using the server.
    Rstr,
    /// Any other code; the packet is just unusable.
    Other([u8; 4]),
}

impl NtpPacket {
    /// A client request whose transmit timestamp is the random `cookie`. The
    /// server echoes it as origin, which proves the reply answers this very
    /// request (the real send time stays local and is not disclosed).
    pub fn client_request(cookie: u64, poll: i8, synced: bool) -> [u8; HEADER_LEN] {
        NtpPacket {
            leap: if synced { 0 } else { LEAP_UNSYNC },
            version: VERSION,
            mode: MODE_CLIENT,
            poll,
            precision: -20,
            transmit_ts: NtpTimestamp(cookie),
            ..Default::default()
        }
        .encode()
    }

    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0] = (self.leap << 6) | ((self.version & 7) << 3) | (self.mode & 7);
        b[1] = self.stratum;
        b[2] = self.poll as u8;
        b[3] = self.precision as u8;
        b[4..8].copy_from_slice(&self.root_delay.to_be_bytes());
        b[8..12].copy_from_slice(&self.root_dispersion.to_be_bytes());
        b[12..16].copy_from_slice(&self.reference_id.to_be_bytes());
        b[16..24].copy_from_slice(&self.reference_ts.0.to_be_bytes());
        b[24..32].copy_from_slice(&self.origin_ts.0.to_be_bytes());
        b[32..40].copy_from_slice(&self.receive_ts.0.to_be_bytes());
        b[40..48].copy_from_slice(&self.transmit_ts.0.to_be_bytes());
        b
    }

    /// Parses the 48-byte header; extension fields and MACs are ignored.
    pub fn decode(b: &[u8]) -> Result<Self, PacketError> {
        if b.len() < HEADER_LEN {
            return Err(PacketError::TooShort);
        }
        let version = (b[0] >> 3) & 7;
        if !(1..=4).contains(&version) {
            return Err(PacketError::BadVersion(version));
        }
        let u32_at = |i: usize| u32::from_be_bytes(b[i..i + 4].try_into().unwrap());
        let u64_at = |i: usize| u64::from_be_bytes(b[i..i + 8].try_into().unwrap());
        Ok(NtpPacket {
            leap: b[0] >> 6,
            version,
            mode: b[0] & 7,
            stratum: b[1],
            poll: b[2] as i8,
            precision: b[3] as i8,
            root_delay: u32_at(4),
            root_dispersion: u32_at(8),
            reference_id: u32_at(12),
            reference_ts: NtpTimestamp(u64_at(16)),
            origin_ts: NtpTimestamp(u64_at(24)),
            receive_ts: NtpTimestamp(u64_at(32)),
            transmit_ts: NtpTimestamp(u64_at(40)),
        })
    }

    /// The Kiss-o'-Death code, when this is a KoD packet (stratum 0 and leap
    /// indicator 3; the reference id carries four ASCII letters).
    pub fn kiss_code(&self) -> Option<KissCode> {
        if self.stratum != 0 || self.leap != LEAP_UNSYNC {
            return None;
        }
        let code = self.reference_id.to_be_bytes();
        Some(match &code {
            b"RATE" => KissCode::Rate,
            b"DENY" => KissCode::Deny,
            b"RSTR" => KissCode::Rstr,
            _ => KissCode::Other(code),
        })
    }

    pub fn root_delay_secs(&self) -> f64 {
        short_to_secs(self.root_delay)
    }

    pub fn root_dispersion_secs(&self) -> f64 {
        short_to_secs(self.root_dispersion)
    }

    /// 2^precision seconds.
    pub fn precision_secs(&self) -> f64 {
        2f64.powi(self.precision as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIX_2026: i64 = 1_791_000_000_000_000_000; // 2026-10-03

    #[test]
    fn timestamp_round_trip_ns_precision() {
        for ns in [0i64, 1, 999_999_999, UNIX_2026 + 123_456_789, UNIX_2026 - 1] {
            let ts = NtpTimestamp::from_unix_ns(ns);
            assert!((ts.to_unix_ns(UNIX_2026) - ns).abs() <= 1, "{ns}");
        }
    }

    #[test]
    fn era_rollover_2036() {
        // 2036-02-07 06:28:16 UTC is NTP second 2^32: era 1 starts.
        let rollover = ((1i64 << 32) - NTP_UNIX_OFFSET_S) * 1_000_000_000;
        let before = rollover - 1_500_000_000;
        let after = rollover + 2_250_000_000;
        let ts_after = NtpTimestamp::from_unix_ns(after);
        assert!(ts_after.0 >> 32 < 10, "era 1 seconds wrap to small values");
        // Pivot before the rollover still lands after it, and the other way round.
        assert_eq!(ts_after.to_unix_ns(before), after);
        assert_eq!(NtpTimestamp::from_unix_ns(before).to_unix_ns(after), before);
        // A pivot 60 years off still resolves the right era.
        assert_eq!(
            ts_after.to_unix_ns(after - 60 * 365 * 86_400 * 1_000_000_000),
            after
        );
    }

    #[test]
    fn known_vector() {
        // 2026-01-01T00:00:00.5Z
        let ns = 1_767_225_600_500_000_000;
        let ts = NtpTimestamp::from_unix_ns(ns);
        assert_eq!(ts.0 >> 32, (1_767_225_600 + NTP_UNIX_OFFSET_S) as u64);
        assert_eq!(ts.0 & 0xFFFF_FFFF, 0x8000_0000);
    }

    #[test]
    fn encode_decode() {
        let p = NtpPacket {
            leap: 1,
            version: 4,
            mode: MODE_SERVER,
            stratum: 2,
            poll: 6,
            precision: -23,
            root_delay: 0x0000_1000,
            root_dispersion: 0x0000_0800,
            reference_id: 0xC000_0201,
            reference_ts: NtpTimestamp(1),
            origin_ts: NtpTimestamp(2),
            receive_ts: NtpTimestamp(3),
            transmit_ts: NtpTimestamp(4),
        };
        let mut bytes = p.encode().to_vec();
        assert_eq!(bytes[0], 0b01_100_100);
        bytes.extend_from_slice(&[0; 20]); // a MAC is ignored
        assert_eq!(NtpPacket::decode(&bytes), Ok(p));
        assert_eq!(p.root_delay_secs(), 1.0 / 16.0);
        assert_eq!(NtpPacket::decode(&bytes[..47]), Err(PacketError::TooShort));
        bytes[0] = 0b00_000_100;
        assert_eq!(NtpPacket::decode(&bytes), Err(PacketError::BadVersion(0)));
    }

    #[test]
    fn client_request_layout() {
        let b = NtpPacket::client_request(0x0123_4567_89AB_CDEF, 6, false);
        assert_eq!(b[0], 0b11_100_011);
        assert_eq!(b[2], 6);
        assert_eq!(b[40..48], 0x0123_4567_89AB_CDEFu64.to_be_bytes());
        assert!(b[16..40].iter().all(|&x| x == 0), "no local time leaks");
    }

    #[test]
    fn kiss_codes() {
        let mut p = NtpPacket {
            leap: 3,
            stratum: 0,
            reference_id: u32::from_be_bytes(*b"RATE"),
            ..Default::default()
        };
        assert_eq!(p.kiss_code(), Some(KissCode::Rate));
        p.reference_id = u32::from_be_bytes(*b"DENY");
        assert_eq!(p.kiss_code(), Some(KissCode::Deny));
        p.reference_id = u32::from_be_bytes(*b"RSTR");
        assert_eq!(p.kiss_code(), Some(KissCode::Rstr));
        p.reference_id = u32::from_be_bytes(*b"INIT");
        assert_eq!(p.kiss_code(), Some(KissCode::Other(*b"INIT")));
        p.stratum = 2;
        assert_eq!(p.kiss_code(), None);
    }
}
