//! Times and names for display. All pure: the clock and timezone come in as arguments so the
//! fake data and the snapshots read the same on every machine.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Local, TimeZone};

pub const DAY: i64 = 86_400;

/// How to turn a UTC instant into the wall clock the user reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tz {
    /// The machine's timezone.
    Local,
    /// A fixed offset from UTC in seconds (scenes and tests).
    Fixed(i32),
}

impl Tz {
    pub fn offset(self, unix: i64) -> i64 {
        match self {
            Tz::Fixed(s) => i64::from(s),
            Tz::Local => Local
                .timestamp_opt(unix, 0)
                .single()
                .map_or(0, |d| i64::from(d.offset().local_minus_utc())),
        }
    }
    /// Seconds since the epoch on the wall clock.
    pub fn local(self, unix: i64) -> i64 {
        unix + self.offset(unix)
    }
}

pub fn unix(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

/// "14:03".
pub fn clock_label(tz: Tz, unix: i64) -> String {
    let l = tz.local(unix).rem_euclid(DAY);
    format!("{:02}:{:02}", l / 3600, (l % 3600) / 60)
}

/// The wall-clock day number of `unix`.
pub fn day_of(tz: Tz, unix: i64) -> i64 {
    tz.local(unix).div_euclid(DAY)
}

/// Seconds from `unix` to the next local midnight (at least 1).
pub fn until_next_day(tz: Tz, unix: i64) -> i64 {
    (DAY - tz.local(unix).rem_euclid(DAY)).max(1)
}

/// Civil date of a day number (days since 1970-01-01): Howard Hinnant's algorithm.
fn civil(day: i64) -> (i64, u32, u32) {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// "Today", "Yesterday", else "Tue, Nov 17" (with the year when it isn't this one).
pub fn day_label(day: i64, today: i64) -> String {
    match today - day {
        0 => return "Today".into(),
        1 => return "Yesterday".into(),
        _ => {}
    }
    const WEEKDAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (y, m, d) = civil(day);
    let wd = WEEKDAYS[day.rem_euclid(7) as usize];
    let base = format!("{wd}, {} {d}", MONTHS[m as usize - 1]);
    if civil(today).0 == y {
        base
    } else {
        format!("{base}, {y}")
    }
}

/// "Nov 17": the month and day of a day number (chart axes).
pub fn short_date(day: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, m, d) = civil(day);
    format!("{} {d}", MONTHS[m as usize - 1])
}

/// The day number of a `YYYY-MM-DD` date.
pub fn parse_date(date: &str) -> Option<i64> {
    let mut parts = date.split('-');
    let (y, m, d) = (
        parts.next()?.parse::<i64>().ok()?,
        parts.next()?.parse::<u32>().ok()?,
        parts.next()?.parse::<u32>().ok()?,
    );
    ((1..=12).contains(&m) && (1..=31).contains(&d) && parts.next().is_none())
        .then(|| utc(y, m, d, 0, 0).div_euclid(DAY))
}

/// A byte count as people read it: "512 B", "1.5 KiB", "38 MiB", "1.2 GiB".
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    // One decimal below 10, none above; a value that rounds up to 1024 moves to the next unit.
    if v >= 1023.5 && unit < UNITS.len() - 1 {
        return format!("1 {}", UNITS[unit + 1]);
    }
    if v < 10.0 {
        format!("{:.1} {}", (v * 10.0).round() / 10.0, UNITS[unit])
    } else {
        format!("{v:.0} {}", UNITS[unit])
    }
}

/// Unix seconds of a wall-clock date and time in UTC (fake data).
pub fn utc(y: i64, m: u32, d: u32, hh: i64, mm: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 719_468) * DAY + hh * 3600 + mm * 60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_round_trip() {
        let t = utc(2026, 11, 17, 9, 5);
        assert_eq!(civil(t.div_euclid(DAY)), (2026, 11, 17));
        assert_eq!(clock_label(Tz::Fixed(0), t), "09:05");
        assert_eq!(clock_label(Tz::Fixed(3600), t), "10:05");
    }

    /// The label a message sent at `sent` gets while the wall clock reads `now`, in `tz`.
    fn label(tz: Tz, sent: i64, now: i64) -> String {
        day_label(day_of(tz, sent), day_of(tz, now))
    }

    #[test]
    fn the_day_turns_at_local_midnight_not_utc() {
        // 2026-11-20 00:30 in UTC+2 is still 2026-11-19 22:30 UTC.
        let tz = Tz::Fixed(2 * 3600);
        let just_after = utc(2026, 11, 19, 22, 30);
        let just_before = utc(2026, 11, 19, 21, 59);
        assert_eq!(label(tz, just_after, just_after), "Today");
        assert_eq!(
            label(tz, just_before, just_after),
            "Yesterday",
            "23:59 local is the day before 00:30 local"
        );
        // The same instants read in UTC are one day, so the label would be wrong there.
        assert_eq!(label(Tz::Fixed(0), just_before, just_after), "Today");
    }

    #[test]
    fn west_of_utc_the_evening_is_still_today() {
        // 2026-11-20 03:00 UTC is 22:00 on the 19th in UTC-5.
        let tz = Tz::Fixed(-5 * 3600);
        let now = utc(2026, 11, 20, 3, 0);
        assert_eq!(label(tz, utc(2026, 11, 19, 15, 0), now), "Today");
        assert_eq!(
            label(tz, utc(2026, 11, 19, 4, 59), now),
            "Yesterday",
            "23:59 on the 18th local"
        );
        assert_eq!(label(tz, utc(2026, 11, 18, 4, 59), now), "Tue, Nov 17");
    }

    #[test]
    fn the_exact_midnight_belongs_to_the_new_day() {
        let tz = Tz::Fixed(3600);
        let midnight = utc(2026, 11, 19, 23, 0); // 00:00 on the 20th local
        assert_eq!(label(tz, midnight, midnight), "Today");
        assert_eq!(label(tz, midnight - 1, midnight), "Yesterday");
        assert_eq!(until_next_day(tz, midnight), DAY);
        assert_eq!(until_next_day(tz, midnight - 1), 1);
    }

    #[test]
    fn byte_sizes_read_naturally() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1024), "1.0 KiB");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(10 * 1024), "10 KiB");
        assert_eq!(bytes(38 * 1024 * 1024 + 300_000), "38 MiB");
        assert_eq!(bytes(1_288_490_189), "1.2 GiB");
        assert_eq!(bytes(1024 * 1024 - 1), "1 MiB");
        assert_eq!(bytes(u64::MAX), "16 EiB");
    }

    #[test]
    fn dates_parse_and_shorten() {
        let day = parse_date("2026-11-17").unwrap();
        assert_eq!(day, utc(2026, 11, 17, 0, 0).div_euclid(DAY));
        assert_eq!(short_date(day), "Nov 17");
        assert_eq!(parse_date("2026-13-01"), None);
        assert_eq!(parse_date("nope"), None);
    }

    #[test]
    fn labels() {
        let today = utc(2026, 11, 19, 12, 0).div_euclid(DAY);
        assert_eq!(day_label(today, today), "Today");
        assert_eq!(day_label(today - 1, today), "Yesterday");
        assert_eq!(day_label(today - 2, today), "Tue, Nov 17");
        assert_eq!(day_label(today - 400, today), "Wed, Oct 15, 2025");
    }
}
