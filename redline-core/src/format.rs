//! Text formatting for durations, sizes and times, shared by jobs and the UI.

/// Phosphor's arrow-right glyph. Job subtitles are stored with it and drawn in the icon font.
pub const ARROW_RIGHT: &str = "\u{E06C}";

pub fn fmt_duration(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Like `fmt_duration` but keeps tenths of a second when the time isn't whole.
pub fn fmt_time(secs: f64) -> String {
    let tenths = (secs.fract() * 10.0).round() as u32;
    if tenths == 0 || tenths >= 10 {
        fmt_duration(secs)
    } else {
        format!("{}.{tenths}", fmt_duration(secs.trunc()))
    }
}

/// Parses `90`, `1:30`, `1:02:03` or `12.5` into seconds.
pub fn parse_duration(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut secs = 0.0;
    for (i, part) in parts.iter().enumerate() {
        let v: f64 = part.trim().parse().ok().filter(|v: &f64| *v >= 0.0)?;
        // Only the last field may be fractional, and minutes/seconds after a colon stay under 60.
        if i + 1 < parts.len() && v.fract() != 0.0 || i > 0 && v >= 60.0 {
            return None;
        }
        secs = secs * 60.0 + v;
    }
    Some(secs)
}

pub fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// "Mar 4, 2026 14:05" in local time.
pub fn fmt_timestamp(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%b %-d, %Y %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// "Starts at 14:05", "Starts tomorrow at 02:00" or a full date further out.
pub fn starts_at(ts: i64) -> String {
    use chrono::{Local, TimeZone};
    let Some(t) = Local.timestamp_opt(ts, 0).single() else {
        return "Scheduled".into();
    };
    let today = Local::now().date_naive();
    let day = t.date_naive();
    if day == today {
        format!("Starts at {}", t.format("%H:%M"))
    } else if day == today.succ_opt().unwrap_or(today) {
        format!("Starts tomorrow at {}", t.format("%H:%M"))
    } else {
        format!("Starts {}", t.format("%b %-d at %H:%M"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_duration(5.0), "0:05");
        assert_eq!(fmt_duration(125.4), "2:05");
        assert_eq!(fmt_duration(3725.0), "1:02:05");
        assert_eq!(fmt_time(65.0), "1:05");
        assert_eq!(fmt_time(65.5), "1:05.5");
        assert_eq!(parse_duration("90"), Some(90.0));
        assert_eq!(parse_duration(" 1:30 "), Some(90.0));
        assert_eq!(parse_duration("1:02:03"), Some(3723.0));
        assert_eq!(parse_duration("0:12.5"), Some(12.5));
        assert_eq!(parse_duration("1:75"), None);
        assert_eq!(parse_duration("1.5:00"), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration(""), None);
        assert_eq!(fmt_bytes(512), "512 B");
        assert_eq!(fmt_bytes(1536), "1.5 KB");
        assert_eq!(fmt_bytes(5 * 1024 * 1024), "5.0 MB");
    }
}
