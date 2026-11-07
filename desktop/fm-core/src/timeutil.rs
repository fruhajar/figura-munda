//! Civil-time arithmetic, with no dependency on a calendar crate.
//!
//! Every format in scope stores time differently (EXIF `2024:06:14 19:42:07`,
//! PDF `D:20240614194207+02'00'`, PNG `tIME` as binary fields, MP4 as seconds
//! since 1904-01-01, ZIP as halved-second MS-DOS fields), so the one shape they
//! all reduce to is a broken-down civil instant plus a coarsening operation.

use serde::{Deserialize, Serialize};

/// A civil date and time, always understood as UTC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Civil {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// The fixed constant the `uniform` profile normalises every timestamp to.
///
/// 1980-01-01 is the MS-DOS epoch, so it is exactly representable in a ZIP
/// member header. Reproducible builds pin entries to the same instant, which is
/// the point: a shared constant makes no false claim and is already common.
pub const EPOCH_1980: Civil = Civil { year: 1980, month: 1, day: 1, hour: 0, minute: 0, second: 0 };

/// Seconds between 1904-01-01 and 1970-01-01, the MP4 / QuickTime offset.
pub const MP4_EPOCH_OFFSET: i64 = 2_082_844_800;

impl Civil {
    pub fn date_only(year: i32, month: u8, day: u8) -> Self {
        Civil { year, month, day, hour: 0, minute: 0, second: 0 }
    }

    /// Truncate to midnight on the same day.
    pub fn coarsen_day(self) -> Self {
        Civil { hour: 0, minute: 0, second: 0, ..self }
    }

    /// Truncate to midnight on the first of the same month.
    pub fn coarsen_month(self) -> Self {
        Civil { day: 1, hour: 0, minute: 0, second: 0, ..self }
    }

    /// Days since 1970-01-01. Hinnant's `days_from_civil`.
    pub fn days_from_epoch(self) -> i64 {
        let y = if self.month <= 2 { self.year - 1 } else { self.year } as i64;
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let m = self.month as i64;
        let d = self.day as i64;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    pub fn unix_seconds(self) -> i64 {
        self.days_from_epoch() * 86_400
            + self.hour as i64 * 3600
            + self.minute as i64 * 60
            + self.second as i64
    }

    /// Inverse of [`Civil::unix_seconds`]. Hinnant's `civil_from_days`.
    pub fn from_unix_seconds(secs: i64) -> Self {
        let days = secs.div_euclid(86_400);
        let rem = secs.rem_euclid(86_400);
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        Civil {
            year: (if m <= 2 { y + 1 } else { y }) as i32,
            month: m as u8,
            day: d as u8,
            hour: (rem / 3600) as u8,
            minute: (rem % 3600 / 60) as u8,
            second: (rem % 60) as u8,
        }
    }

    /// `2024:06:14 19:42:07`, the EXIF ASCII form.
    pub fn to_exif(self) -> String {
        format!(
            "{:04}:{:02}:{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// `2024-06-14T19:42:07Z`, the XMP / ISO form.
    pub fn to_iso(self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// `D:20240614194207Z`, the PDF form. Always emitted with an explicit zone:
    /// a bare local time is itself a weak location signal.
    pub fn to_pdf(self) -> String {
        format!(
            "D:{:04}{:02}{:02}{:02}{:02}{:02}Z",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// `2024-06-14`, the Dublin Core date form.
    pub fn to_date(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    /// MS-DOS date and time, as stored in a ZIP member header. Seconds are
    /// halved, so odd seconds are not representable; this rounds down, which is
    /// what every ZIP writer does.
    pub fn to_dos(self) -> (u16, u16) {
        let date = (((self.year - 1980).max(0) as u16) << 9)
            | ((self.month as u16) << 5)
            | self.day as u16;
        let time = ((self.hour as u16) << 11) | ((self.minute as u16) << 5) | (self.second as u16 / 2);
        (date, time)
    }

    pub fn from_dos(date: u16, time: u16) -> Self {
        Civil {
            year: 1980 + (date >> 9) as i32,
            month: ((date >> 5) & 0xF) as u8,
            day: (date & 0x1F) as u8,
            hour: (time >> 11) as u8,
            minute: ((time >> 5) & 0x3F) as u8,
            second: ((time & 0x1F) * 2) as u8,
        }
    }
}

/// Pull a civil instant out of whatever a field happens to hold.
///
/// Deliberately permissive about separators and trailing zone text: the point is
/// to recognise a timestamp well enough to coarsen it, not to validate it. A
/// zone suffix is discarded rather than applied, so a coarsened value can be off
/// by a day at the margin; that is acceptable for a field whose precision is
/// being destroyed on purpose, and `normalise` avoids the question entirely.
pub fn parse_loose(s: &str) -> Option<Civil> {
    let digits: Vec<u8> = s
        .bytes()
        .skip_while(|b| !b.is_ascii_digit())
        .filter(|b| b.is_ascii_digit())
        .collect();
    if digits.len() < 8 {
        return None;
    }
    let n = |range: std::ops::Range<usize>| -> i32 {
        digits[range].iter().fold(0i32, |a, b| a * 10 + (b - b'0') as i32)
    };
    let get = |range: std::ops::Range<usize>, default: i32| -> i32 {
        if digits.len() >= range.end { n(range) } else { default }
    };
    let year = n(0..4);
    if !(1800..=2200).contains(&year) {
        return None;
    }
    let month = n(4..6);
    let day = n(6..8);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(Civil {
        year,
        month: month as u8,
        day: day as u8,
        hour: get(8..10, 0).clamp(0, 23) as u8,
        minute: get(10..12, 0).clamp(0, 59) as u8,
        second: get(12..14, 0).clamp(0, 59) as u8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_unix_seconds() {
        let c = Civil { year: 2024, month: 6, day: 14, hour: 19, minute: 42, second: 7 };
        assert_eq!(c.unix_seconds(), 1_718_394_127);
        assert_eq!(Civil::from_unix_seconds(1_718_394_127), c);
    }

    #[test]
    fn mp4_epoch_matches_the_fixture() {
        // manifest.json declares mvhd creation_time 3801238927 for the same instant.
        let c = Civil { year: 2024, month: 6, day: 14, hour: 19, minute: 42, second: 7 };
        assert_eq!(c.unix_seconds() + MP4_EPOCH_OFFSET, 3_801_238_927);
    }

    #[test]
    fn dos_time_halves_seconds() {
        // The fixture README's first finding: 09:26:53 is unrepresentable.
        let c = Civil { year: 2019, month: 3, day: 14, hour: 9, minute: 26, second: 53 };
        let (d, t) = c.to_dos();
        assert_eq!(Civil::from_dos(d, t).second, 52);
    }

    #[test]
    fn epoch_1980_is_dos_representable() {
        let (d, t) = EPOCH_1980.to_dos();
        assert_eq!((d, t), (0x0021, 0));
        assert_eq!(Civil::from_dos(d, t), EPOCH_1980);
    }

    #[test]
    fn parses_every_in_scope_spelling() {
        let want = Civil { year: 2024, month: 6, day: 14, hour: 19, minute: 42, second: 7 };
        for s in [
            "2024:06:14 19:42:07",
            "2024-06-14T19:42:07Z",
            "D:20240614194207+02'00'",
            "2024-06-14 19:42:07",
        ] {
            assert_eq!(parse_loose(s), Some(want), "{s}");
        }
        assert_eq!(parse_loose("2024-06-14"), Some(Civil::date_only(2024, 6, 14)));
        assert_eq!(parse_loose("not a date"), None);
        assert_eq!(parse_loose("17.4.1"), None, "a version string is not a timestamp");
    }

    #[test]
    fn coarsening_truncates_rather_than_rounds() {
        let c = Civil { year: 2024, month: 6, day: 14, hour: 19, minute: 42, second: 7 };
        assert_eq!(c.coarsen_day().to_iso(), "2024-06-14T00:00:00Z");
        assert_eq!(c.coarsen_month().to_iso(), "2024-06-01T00:00:00Z");
    }
}
