//! Date helpers and the month grid for the calendar drop-downs (no date crate).
use crate::CalDay;

pub type Ymd = (i64, u32, u32);

pub const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June",
    "July", "August", "September", "October", "November", "December",
];

/// Days since 1970-01-01 for a calendar date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn month_len(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 },
    }
}

/// 0 = Monday ... 6 = Sunday (1970-01-01 was a Thursday).
pub fn weekday_mon0(y: i64, m: u32, d: u32) -> usize {
    (days_from_civil(y, m, d) + 3).rem_euclid(7) as usize
}

pub fn fmt(d: Ymd) -> String {
    format!("{:04}-{:02}-{:02}", d.0, d.1, d.2)
}

pub fn title(shown: (i64, u32)) -> String {
    format!("{} {}", MONTHS[(shown.1 - 1) as usize], shown.0)
}

/// Move the shown month by `by`, never past the month of `today`.
pub fn step(shown: (i64, u32), by: i32, today: Ymd) -> (i64, u32) {
    let t = shown.0 * 12 + (shown.1 as i64 - 1) + by as i64;
    let next = (t.div_euclid(12), (t.rem_euclid(12) + 1) as u32);
    if next <= (today.0, today.1) { next } else { shown }
}

/// 42 cells (6 weeks, Monday first) for the month `shown`. Days after `today` are disabled.
pub fn cells(shown: (i64, u32), sel: Ymd, today: Ymd) -> Vec<CalDay> {
    let (y, m) = shown;
    let first = weekday_mon0(y, m, 1) as i64;
    let len = month_len(y, m) as i64;
    (0..42i64)
        .map(|i| {
            let day = i - first + 1;
            if day < 1 || day > len {
                return CalDay { day: 0, enabled: false, selected: false, today: false };
            }
            let ymd = (y, m, day as u32);
            CalDay { day: day as i32, enabled: ymd <= today, selected: ymd == sel, today: ymd == today }
        })
        .collect()
}

/// "Local seconds" of a date and time: seconds since 1970-01-01 00:00:00 on the
/// *wall clock* (no time zone). The database stores wall-clock text, so plain
/// arithmetic on these numbers is all we need.
pub fn naive_secs(d: Ymd, h: i32, m: i32, s: i32) -> i64 {
    days_from_civil(d.0, d.1, d.2) * 86_400 + h as i64 * 3600 + m as i64 * 60 + s as i64
}

pub fn fmt_naive(t: i64) -> String {
    let (y, m, d) = crate::civil_from_days(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d, s / 3600, (s % 3600) / 60, s % 60)
}

/// "YYYY-MM-DD HH:MM:SS" -> naive seconds.
pub fn parse_naive(ts: &str) -> Option<i64> {
    let b = ts.as_bytes();
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let n = |a: usize, z: usize| ts.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0, 4)?, n(5, 7)? as u32, n(8, 10)? as u32);
    let (h, mi, s) = (n(11, 13)?, n(14, 16)?, n(17, 19)?);
    if !(1..=12).contains(&mo) || d < 1 || d > 31 || h > 23 || mi > 59 || s > 60 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekday_and_lengths() {
        assert_eq!(weekday_mon0(2026, 11, 22), 6); // Sunday
        assert_eq!(weekday_mon0(1970, 1, 1), 3); // Thursday
        assert_eq!(month_len(2024, 2), 29);
        assert_eq!(month_len(2026, 2), 28);
    }

    #[test]
    fn naive_round_trip() {
        let t = naive_secs((2026, 11, 22), 10, 5, 9);
        assert_eq!(fmt_naive(t), "2026-11-22 10:05:09");
        assert_eq!(parse_naive("2026-11-22 10:05:09"), Some(t));
        assert_eq!(parse_naive("garbage"), None);
    }

    #[test]
    fn grid_marks_days() {
        let c = cells((2026, 11), (2026, 11, 15), (2026, 11, 22));
        assert_eq!(c.len(), 42);
        assert_eq!(c.iter().filter(|d| d.day > 0).count(), 30);
        assert!(c.iter().find(|d| d.day == 15).unwrap().selected);
        assert!(!c.iter().find(|d| d.day == 23).unwrap().enabled);
        assert_eq!(c[6].day, 1); // 1 Nov 2026 is a Sunday: 7th cell
    }

    #[test]
    fn month_step_stops_at_today() {
        assert_eq!(step((2026, 11), 1, (2026, 11, 22)), (2026, 11));
        assert_eq!(step((2026, 1), -1, (2026, 11, 22)), (2025, 12));
    }
}
