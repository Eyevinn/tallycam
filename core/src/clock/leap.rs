//! TAI - UTC leap second table, the equivalent of chrony's
//! `leapsectz right/UTC`.
//!
//! The disciplined clock runs on TAI, which never jumps. UTC is derived from
//! it with this table. A leap second announced by the NTP sources (leap
//! indicator, majority of the selected sources) and not yet in the table is
//! added at runtime as [`DynamicLeap`].

/// (UTC Unix seconds when the offset takes effect, TAI - UTC in seconds),
/// from the IERS leap second list.
pub const LEAP_TABLE: [(i64, i32); 28] = [
    (63_072_000, 10),    // 1972-01-01
    (78_796_800, 11),    // 1972-07-01
    (94_694_400, 12),    // 1973-01-01
    (126_230_400, 13),   // 1974-01-01
    (157_766_400, 14),   // 1975-01-01
    (189_302_400, 15),   // 1976-01-01
    (220_924_800, 16),   // 1977-01-01
    (252_460_800, 17),   // 1978-01-01
    (283_996_800, 18),   // 1979-01-01
    (315_532_800, 19),   // 1980-01-01
    (362_793_600, 20),   // 1981-07-01
    (394_329_600, 21),   // 1982-07-01
    (425_865_600, 22),   // 1983-07-01
    (489_024_000, 23),   // 1985-07-01
    (567_993_600, 24),   // 1988-01-01
    (631_152_000, 25),   // 1990-01-01
    (662_688_000, 26),   // 1991-01-01
    (709_948_800, 27),   // 1992-07-01
    (741_484_800, 28),   // 1993-07-01
    (773_020_800, 29),   // 1994-07-01
    (820_454_400, 30),   // 1996-01-01
    (867_715_200, 31),   // 1997-07-01
    (915_148_800, 32),   // 1999-01-01
    (1_136_073_600, 33), // 2006-01-01
    (1_230_768_000, 34), // 2009-01-01
    (1_341_100_800, 35), // 2012-07-01
    (1_435_708_800, 36), // 2015-07-01
    (1_483_228_800, 37), // 2017-01-01
];

/// TAI - UTC for good: IERS Bulletin C 72 (2026-07-06) announced no leap
/// second at the end of 2026, and the CGPM decided on 2026-10-15 that UTC
/// has no leap seconds from May 2027. The table needs no expiry and no more
/// updates. As Strom's clock health expects.
pub const EXPECTED_TAI_OFFSET_S: i32 = 37;

const NS: i64 = 1_000_000_000;
const DAY: i64 = 86_400;

/// A leap second learned at runtime: from `utc_s` on, TAI - UTC is `offset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DynamicLeap {
    /// 0 when none.
    pub utc_s: i64,
    pub offset: i32,
}

/// TAI - UTC in seconds at the UTC instant `utc_s` (Unix seconds).
pub fn tai_utc_at_utc(utc_s: i64, extra: DynamicLeap) -> i32 {
    if extra.utc_s != 0 && utc_s >= extra.utc_s {
        return extra.offset;
    }
    let mut offset = 10;
    for &(at, o) in LEAP_TABLE.iter() {
        if utc_s >= at {
            offset = o;
        } else {
            break;
        }
    }
    offset
}

/// UTC Unix ns to TAI ns since the PTP epoch (1970-01-01 00:00:00 TAI).
pub fn utc_to_tai_ns(utc_ns: i64, extra: DynamicLeap) -> i64 {
    utc_ns + tai_utc_at_utc(utc_ns.div_euclid(NS), extra) as i64 * NS
}

/// TAI ns to UTC Unix ns. During an inserted leap second (23:59:60) the
/// result repeats 23:59:59, like a POSIX clock.
pub fn tai_to_utc_ns(tai_ns: i64, extra: DynamicLeap) -> i64 {
    tai_ns - tai_utc_at_tai(tai_ns, extra) as i64 * NS
}

/// TAI - UTC at a TAI instant. An entry applies from the start of its leap
/// second, (UTC of the entry + previous offset) in TAI.
pub fn tai_utc_at_tai(tai_ns: i64, extra: DynamicLeap) -> i32 {
    let tai_s = tai_ns.div_euclid(NS);
    let mut offset = 10;
    let mut prev = 10;
    for &(at, o) in LEAP_TABLE.iter() {
        if tai_s >= at + prev as i64 {
            offset = o;
            prev = o;
        } else {
            break;
        }
    }
    if extra.utc_s != 0 && tai_s >= extra.utc_s + offset as i64 {
        offset = extra.offset;
    }
    offset
}

/// Leap second scheduled at the end of the UTC day containing `utc_s`:
/// +1 insert, -1 delete, 0 none (from the table or the dynamic entry).
pub fn leap_at_end_of_day(utc_s: i64, extra: DynamicLeap) -> i32 {
    let midnight = (utc_s.div_euclid(DAY) + 1) * DAY;
    tai_utc_at_utc(midnight, extra) - tai_utc_at_utc(midnight - 1, extra)
}

/// Leap seconds are only allowed at the end of June 30 and December 31 UTC
/// (chrony's `is_leap_second_day`).
pub fn is_leap_second_day(utc_s: i64) -> bool {
    let (_, month, day) = civil_from_days(utc_s.div_euclid(DAY));
    (month == 6 && day == 30) || (month == 12 && day == 31)
}

/// The next UTC midnight after `utc_s`.
pub fn next_midnight(utc_s: i64) -> i64 {
    (utc_s.div_euclid(DAY) + 1) * DAY
}

/// True within 5 s of a leap second (chrony's `LEAP_SECOND_CLOSE`): samples
/// taken then are dropped because the server may or may not have applied it.
pub fn is_leap_close(utc_s: i64, extra: DynamicLeap) -> bool {
    const CLOSE: i64 = 5;
    let midnight = (utc_s + CLOSE).div_euclid(DAY) * DAY;
    (utc_s - midnight).abs() < CLOSE
        && tai_utc_at_utc(midnight, extra) != tai_utc_at_utc(midnight - 1, extra)
}

/// (year, month 1-12, day 1-31) from days since 1970-01-01 (Howard Hinnant's
/// algorithm).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `YYYY-MM-DD hh:mm:ss` for logs.
pub fn format_utc(utc_s: i64) -> String {
    let (y, m, d) = civil_from_days(utc_s.div_euclid(DAY));
    let s = utc_s.rem_euclid(DAY);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: DynamicLeap = DynamicLeap {
        utc_s: 0,
        offset: 0,
    };

    #[test]
    fn offset_is_37_in_2026() {
        assert_eq!(tai_utc_at_utc(1_791_000_000, NONE), EXPECTED_TAI_OFFSET_S);
        let utc = 1_791_000_000_123_456_789;
        let tai = utc_to_tai_ns(utc, NONE);
        assert_eq!(tai - utc, 37 * NS);
        assert_eq!(tai_to_utc_ns(tai, NONE), utc);
    }

    #[test]
    fn across_the_2016_leap_second() {
        let leap = 1_483_228_800; // 2017-01-01 00:00:00 UTC
        assert_eq!(tai_utc_at_utc(leap - 1, NONE), 36);
        assert_eq!(tai_utc_at_utc(leap, NONE), 37);
        // TAI runs on through 23:59:60; UTC repeats 23:59:59.
        let tai_2359_59 = (leap - 1 + 36) * NS;
        assert_eq!(
            tai_to_utc_ns(tai_2359_59 + NS / 2, NONE),
            (leap - 1) * NS + NS / 2
        );
        assert_eq!(
            tai_to_utc_ns(tai_2359_59 + NS + NS / 2, NONE),
            (leap - 1) * NS + NS / 2
        );
        assert_eq!(tai_to_utc_ns(tai_2359_59 + 2 * NS, NONE), leap * NS);
        assert_eq!(leap_at_end_of_day(leap - 3600, NONE), 1);
        assert_eq!(leap_at_end_of_day(leap + 3600, NONE), 0);
        assert!(is_leap_close(leap - 2, NONE));
        assert!(is_leap_close(leap + 4, NONE));
        assert!(!is_leap_close(leap + 6, NONE));
        assert!(!is_leap_close(leap - 86_400, NONE));
    }

    #[test]
    fn dynamic_leap_from_the_sources() {
        let end_2026 = 1_798_761_600; // 2027-01-01
        let extra = DynamicLeap {
            utc_s: end_2026,
            offset: 38,
        };
        assert_eq!(tai_utc_at_utc(end_2026 - 1, extra), 37);
        assert_eq!(tai_utc_at_utc(end_2026, extra), 38);
        let tai = utc_to_tai_ns(end_2026 * NS, extra);
        assert_eq!(tai_to_utc_ns(tai, extra), end_2026 * NS);
        assert_eq!(leap_at_end_of_day(end_2026 - 10, extra), 1);
        assert!(is_leap_second_day(end_2026 - 10));
        assert!(!is_leap_second_day(end_2026 - 86_400 - 10));
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(format_utc(1_483_228_799), "2016-12-31 23:59:59");
        assert_eq!(format_utc(1_782_777_600), "2026-06-30 00:00:00");
    }
}
