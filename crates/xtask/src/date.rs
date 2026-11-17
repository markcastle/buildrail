//! Calendar dates for changelog headings, without a date library.
//!
//! The changelog needs exactly two things: today's date in UTC as
//! `YYYY-MM-DD`, and a check that a heading's date is a real one.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// A day in the proleptic Gregorian calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Date {
    /// Four-digit year.
    pub year: i64,
    /// 1 to 12.
    pub month: u32,
    /// 1 to 31, depending on the month.
    pub day: u32,
}

impl Date {
    /// Parses `YYYY-MM-DD` and refuses impossible days such as 2026-02-30.
    pub fn parse(text: &str) -> Result<Self, String> {
        let refuse = || format!("\"{text}\" is not a date. Use YYYY-MM-DD, such as 2026-11-17.");
        let bytes = text.as_bytes();
        let shape_ok = bytes.len() == 10
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes
                .iter()
                .enumerate()
                .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
        if !shape_ok {
            return Err(refuse());
        }
        let number =
            |range: std::ops::Range<usize>| text[range].parse::<u32>().map_err(|_| refuse());
        let year = i64::from(number(0..4)?);
        let month = number(5..7)?;
        let day = number(8..10)?;
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return Err(refuse());
        }
        Ok(Self { year, month, day })
    }

    /// The date `days` days after 1970-01-01.
    pub fn from_days_since_epoch(days: i64) -> Self {
        // Howard Hinnant's civil_from_days, which is exact for every day in
        // the proleptic Gregorian calendar.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        // Both are small and positive by construction.
        Self {
            year,
            month: u32::try_from(month).unwrap_or(1),
            day: u32::try_from(day).unwrap_or(1),
        }
    }

    /// Today in UTC, which is what the changelog's dates mean.
    pub fn today_utc() -> Self {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self::from_days_since_epoch(i64::try_from(seconds / 86_400).unwrap_or(0))
    }
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn parses_and_prints() {
        let date = Date::parse("2026-11-17").unwrap();
        assert_eq!((date.year, date.month, date.day), (2026, 11, 17));
        assert_eq!(date.to_string(), "2026-11-17");
    }

    #[test]
    fn knows_leap_years() {
        assert!(Date::parse("2024-02-29").is_ok());
        assert!(Date::parse("2000-02-29").is_ok());
        assert!(Date::parse("2026-02-29").is_err());
        assert!(Date::parse("1900-02-29").is_err());
        assert!(Date::parse("2026-04-31").is_err());
        assert!(Date::parse("2026-12-31").is_ok());
    }

    #[test]
    fn refuses_bad_shapes() {
        for bad in [
            "",
            "2026-1-17",
            "2026/11/17",
            "17-11-2026",
            "2026-13-01",
            "2026-00-10",
            "2026-11-00",
            "2026-11-17x",
            "２０２６-11-17",
        ] {
            assert!(Date::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn counts_days_from_the_epoch() {
        assert_eq!(Date::from_days_since_epoch(0).to_string(), "1970-01-01");
        assert_eq!(Date::from_days_since_epoch(59).to_string(), "1970-03-01");
        assert_eq!(
            Date::from_days_since_epoch(11_016).to_string(),
            "2000-02-29"
        );
        assert_eq!(
            Date::from_days_since_epoch(20_774).to_string(),
            "2026-11-17"
        );
        assert_eq!(Date::from_days_since_epoch(-1).to_string(), "1969-12-31");
    }

    #[test]
    fn today_is_a_real_date() {
        let today = Date::today_utc();
        assert!(Date::parse(&today.to_string()).is_ok());
        assert!(today.year >= 2026);
    }

    #[test]
    fn orders_chronologically() {
        assert!(Date::parse("2026-11-17").unwrap() > Date::parse("2026-11-16").unwrap());
        assert!(Date::parse("2027-01-01").unwrap() > Date::parse("2026-12-31").unwrap());
    }
}
